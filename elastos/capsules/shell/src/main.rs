//! ElastOS Shell Capsule
//!
//! Pluggable decision engine for capability requests.
//! Modes: cli (interactive prompt, default with TTY), agent (policy rules,
//! default without TTY), auto (grant all — explicit opt-in only).
//! Runs as a long-lived process spawned by the runtime.

use serde::Deserialize;
use std::collections::HashMap;
use std::io::IsTerminal;
use std::time::Instant;

const SHELL_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};

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
        match base64::engine::general_purpose::STANDARD.decode(payload_b64) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(decoded) if !decoded.is_empty() => return Some(decoded),
                Ok(_) => return None,
                Err(e) => {
                    eprintln!("shell: ELASTOS_COMMAND_B64 decoded to invalid utf-8: {}", e);
                    std::process::exit(1);
                }
            },
            Err(e) => {
                eprintln!("shell: failed to decode ELASTOS_COMMAND_B64: {}", e);
                std::process::exit(1);
            }
        }
    }

    None
}

// ---------------------------------------------------------------------------
// API types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct PendingResponse {
    requests: Vec<PendingRequest>,
}

#[derive(Deserialize)]
struct PendingRequest {
    request_id: String,
    #[serde(default)]
    session_id: String,
    #[serde(default)]
    resource: String,
    #[serde(default)]
    action: String,
    #[serde(default)]
    requested_at: u64,
    #[serde(default)]
    expires_at: u64,
}

// ---------------------------------------------------------------------------
// Decision types
// ---------------------------------------------------------------------------

struct DecisionRequest {
    _request_id: String,
    session_id: String,
    resource: String,
    action: String,
    _requested_at: u64,
    _expires_at: u64,
}

#[derive(Debug, PartialEq)]
enum DecisionOutcome {
    Grant,
    Deny,
    Defer,
}

struct DecisionResponse {
    outcome: DecisionOutcome,
    duration: String,
    rationale: String,
}

// ---------------------------------------------------------------------------
// DecisionEngine trait
// ---------------------------------------------------------------------------

trait DecisionEngine {
    fn decide(&mut self, request: &DecisionRequest) -> DecisionResponse;
}

// ---------------------------------------------------------------------------
// AutoGrantEngine — default, behavior-preserving
// ---------------------------------------------------------------------------

struct AutoGrantEngine;

impl DecisionEngine for AutoGrantEngine {
    fn decide(&mut self, _request: &DecisionRequest) -> DecisionResponse {
        DecisionResponse {
            outcome: DecisionOutcome::Grant,
            duration: "session".into(),
            rationale: "Auto-grant: all requests approved".into(),
        }
    }
}

// ---------------------------------------------------------------------------
// CliPromptEngine — interactive terminal prompt
// ---------------------------------------------------------------------------

struct CliPromptEngine;

impl CliPromptEngine {
    fn new() -> Self {
        Self
    }
}

impl DecisionEngine for CliPromptEngine {
    fn decide(&mut self, request: &DecisionRequest) -> DecisionResponse {
        eprintln!(
            "\n[capability] {} requests {} on {}",
            request.session_id, request.action, request.resource
        );
        eprintln!("  [g] grant session  [o] grant once  [d] deny  [s] skip");
        eprint!("  > ");

        let mut input = String::new();
        if std::io::stdin().read_line(&mut input).is_err() {
            return DecisionResponse {
                outcome: DecisionOutcome::Defer,
                duration: String::new(),
                rationale: "Failed to read stdin".into(),
            };
        }

        match input.trim() {
            "g" => DecisionResponse {
                outcome: DecisionOutcome::Grant,
                duration: "session".into(),
                rationale: "Granted by user via CLI (session)".into(),
            },
            "o" => DecisionResponse {
                outcome: DecisionOutcome::Grant,
                duration: "once".into(),
                rationale: "Granted by user via CLI (once)".into(),
            },
            "d" => DecisionResponse {
                outcome: DecisionOutcome::Deny,
                duration: String::new(),
                rationale: "Denied by user via CLI".into(),
            },
            _ => DecisionResponse {
                outcome: DecisionOutcome::Defer,
                duration: String::new(),
                rationale: "Deferred by user via CLI".into(),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// PolicyRulesEngine — rule-based approval (agent mode)
// ---------------------------------------------------------------------------
// Reads rules from $ELASTOS_POLICY_FILE (JSON) or ~/.local/share/elastos/policy.json.
// Format: { "allow": ["elastos://peer/*", "elastos://did/*", "localhost://Users/*"] }
// Requests matching an allow pattern are granted. Others are denied.
// If no policy file exists, all requests are denied (fail-closed).

struct PolicyRulesEngine {
    allow_patterns: Vec<String>,
}

impl PolicyRulesEngine {
    fn load() -> Self {
        let path = std::env::var("ELASTOS_POLICY_FILE").unwrap_or_else(|_| {
            let data_dir = std::env::var("HOME")
                .map(|h| format!("{}/.local/share/elastos/policy.json", h))
                .unwrap_or_else(|_| "/tmp/elastos/policy.json".into());
            data_dir
        });

        let (allow_patterns, source) = match std::fs::read_to_string(&path) {
            Ok(data) => {
                if let Ok(policy) = serde_json::from_str::<serde_json::Value>(&data) {
                    let patterns = policy
                        .get("allow")
                        .and_then(|a| a.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    (patterns, format!("file:{}", path))
                } else {
                    eprintln!(
                        "shell: policy file {} is not valid JSON, using built-in defaults",
                        path
                    );
                    (Self::builtin_defaults(), "built-in".into())
                }
            }
            Err(_) => (Self::builtin_defaults(), "built-in".into()),
        };

        eprintln!(
            "shell: agent mode — {} allow rules ({})",
            allow_patterns.len(),
            source
        );

        Self { allow_patterns }
    }

    /// Built-in default allow rules for agent mode when no policy file exists.
    /// Covers the core provider schemes that standard capsules need.
    fn builtin_defaults() -> Vec<String> {
        vec![
            "elastos://peer/*".into(),
            "elastos://did/*".into(),
            "localhost://Users/*".into(),
            "localhost://UsersAI/*".into(),
            "localhost://Public/*".into(),
            "localhost://MyWebSite/*".into(),
            "localhost://Local/*".into(),
            "localhost://ElastOS/SystemServices/*".into(),
        ]
    }

    fn matches(&self, resource: &str) -> bool {
        self.allow_patterns.iter().any(|pattern| {
            if pattern.ends_with('*') {
                resource.starts_with(pattern.trim_end_matches('*'))
            } else {
                resource == pattern
            }
        })
    }
}

impl DecisionEngine for PolicyRulesEngine {
    fn decide(&mut self, request: &DecisionRequest) -> DecisionResponse {
        if self.matches(&request.resource) {
            DecisionResponse {
                outcome: DecisionOutcome::Grant,
                duration: "session".into(),
                rationale: format!("Policy: allowed by rule matching {}", request.resource),
            }
        } else {
            DecisionResponse {
                outcome: DecisionOutcome::Deny,
                duration: String::new(),
                rationale: format!("Policy: no allow rule for {}", request.resource),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// HTTP helpers
// ---------------------------------------------------------------------------

use std::sync::OnceLock;
use std::time::Duration;

fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .max_idle_connections(4)
            .max_idle_connections_per_host(2)
            .timeout_connect(Duration::from_secs(5))
            .timeout_read(Duration::from_secs(120))
            .build()
    })
}

fn poll_pending(api: &str, token: &str) -> Vec<PendingRequest> {
    let resp = agent()
        .get(&format!("{}/api/capability/pending", api))
        .set("Authorization", &format!("Bearer {}", token))
        .call();

    match resp {
        Ok(resp) => match resp.into_json::<PendingResponse>() {
            Ok(p) => p.requests,
            Err(_) => Vec::new(),
        },
        Err(_) => Vec::new(),
    }
}

fn post_grant(api: &str, token: &str, id: &str, duration: &str, rationale: &str) -> bool {
    let body = serde_json::json!({
        "request_id": id,
        "duration": duration,
        "rationale": rationale,
    });
    match agent()
        .post(&format!("{}/api/capability/grant", api))
        .set("Authorization", &format!("Bearer {}", token))
        .set("Content-Type", "application/json")
        .send_string(&body.to_string())
    {
        Ok(_) => true,
        Err(e) => {
            eprintln!("shell: grant {} failed: {}", id, e);
            false
        }
    }
}

fn post_deny(api: &str, token: &str, id: &str, rationale: &str) -> bool {
    let body = serde_json::json!({
        "request_id": id,
        "reason": rationale,
    });
    match agent()
        .post(&format!("{}/api/capability/deny", api))
        .set("Authorization", &format!("Bearer {}", token))
        .set("Content-Type", "application/json")
        .send_string(&body.to_string())
    {
        Ok(_) => true,
        Err(e) => {
            eprintln!("shell: deny {} failed: {}", id, e);
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

/// Cooldown before re-prompting a deferred request (seconds).
const DEFER_COOLDOWN_SECS: u64 = 5;

/// Resolve raw mode string to effective mode, applying TTY gating.
///
/// Default (no ELASTOS_SHELL_MODE set):
///   - TTY present  → cli  (interactive operator approval)
///   - no TTY       → agent (policy-file rules, fail-closed)
///
/// Explicit modes: "auto", "cli", "agent" are respected as-is,
/// except "cli" without a TTY falls back to "agent" (not auto).
fn resolve_mode(raw: &str, stdin_is_tty: bool) -> &'static str {
    match raw {
        "auto" => "auto",
        "cli" if stdin_is_tty => "cli",
        "cli" => "agent", // cli without TTY → agent (fail-closed), not auto
        "agent" => "agent",
        _ => {
            // Default: secure by default
            if stdin_is_tty {
                "cli"
            } else {
                "agent"
            }
        }
    }
}

fn main() {
    eprintln!("shell: starting v{}", SHELL_VERSION);

    let api = std::env::var("ELASTOS_API").unwrap_or_else(|_| {
        eprintln!("shell: ELASTOS_API not set");
        std::process::exit(1);
    });
    let token = std::env::var("ELASTOS_TOKEN").unwrap_or_else(|_| {
        eprintln!("shell: ELASTOS_TOKEN not set");
        std::process::exit(1);
    });

    // Command dispatch mode: if ELASTOS_COMMAND is set, dispatch it.
    // This is set by the supervisor when forwarding CLI commands to the shell VM.
    if let Some(command_json) = forwarded_command_payload() {
        dispatch_command(&api, &token, &command_json);
        return;
    }

    let raw_mode = std::env::var("ELASTOS_SHELL_MODE").unwrap_or_default();
    let is_tty = std::io::stdin().is_terminal();
    let effective_mode = resolve_mode(&raw_mode, is_tty);

    if raw_mode == "cli" && effective_mode != "cli" {
        eprintln!("shell: cli mode requested but stdin is not a TTY, using agent mode");
    }

    let mut engine: Box<dyn DecisionEngine> = match effective_mode {
        "cli" => Box::new(CliPromptEngine::new()),
        "agent" => Box::new(PolicyRulesEngine::load()),
        _ => Box::new(AutoGrantEngine),
    };

    eprintln!(
        "shell: v{} mode={}, polling {}/api/capability/pending",
        SHELL_VERSION, effective_mode, api
    );
    if effective_mode == "auto" {
        eprintln!(
            "shell: WARNING — auto-grant mode explicitly enabled. All capability requests are approved."
        );
        eprintln!("shell: This bypasses all capability policy. Use only for development.");
    }

    // Cooldown map: request_id → when it was last acted on.
    // Entries are cleared when the request disappears from the pending list.
    let mut cooldown: HashMap<String, Instant> = HashMap::new();

    loop {
        let pending = poll_pending(&api, &token);

        // Collect current pending IDs to prune stale cooldowns
        let pending_ids: std::collections::HashSet<&str> =
            pending.iter().map(|r| r.request_id.as_str()).collect();

        // Prune cooldowns for requests no longer pending
        cooldown.retain(|id, _| pending_ids.contains(id.as_str()));

        for req in &pending {
            // Skip if in cooldown
            if let Some(last) = cooldown.get(&req.request_id) {
                if last.elapsed().as_secs() < DEFER_COOLDOWN_SECS {
                    continue;
                }
            }

            let decision_req = DecisionRequest {
                _request_id: req.request_id.clone(),
                session_id: req.session_id.clone(),
                resource: req.resource.clone(),
                action: req.action.clone(),
                _requested_at: req.requested_at,
                _expires_at: req.expires_at,
            };

            let response = engine.decide(&decision_req);

            match response.outcome {
                DecisionOutcome::Grant => {
                    eprintln!(
                        "shell: GRANT {} {} for {} ({})",
                        req.action, req.resource, req.session_id, response.rationale
                    );
                    if post_grant(
                        &api,
                        &token,
                        &req.request_id,
                        &response.duration,
                        &response.rationale,
                    ) {
                        cooldown.insert(req.request_id.clone(), Instant::now());
                    }
                }
                DecisionOutcome::Deny => {
                    eprintln!(
                        "shell: DENY {} {} for {} ({})",
                        req.action, req.resource, req.session_id, response.rationale
                    );
                    if post_deny(&api, &token, &req.request_id, &response.rationale) {
                        cooldown.insert(req.request_id.clone(), Instant::now());
                    }
                }
                DecisionOutcome::Defer => {
                    cooldown.insert(req.request_id.clone(), Instant::now());
                }
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

// ---------------------------------------------------------------------------
// Orchestrator — dependency resolution, command dispatch, capsule lifecycle
// ---------------------------------------------------------------------------

/// A capsule manifest for dependency resolution (minimal subset).
#[derive(Deserialize, Clone)]
#[cfg(test)]
struct CapsuleSpec {
    _name: String,
    #[serde(default)]
    requires: Vec<CapsuleRequirement>,
    #[serde(default)]
    _provides: Option<String>,
}

#[derive(Deserialize, Clone)]
#[cfg(test)]
struct CapsuleRequirement {
    name: String,
    kind: String,
}

/// Resolve dependencies for a capsule using BFS.
/// Returns capsules in launch order (dependencies first).
#[cfg(test)]
fn resolve_dependencies(
    target: &str,
    specs: &HashMap<String, CapsuleSpec>,
) -> Result<Vec<String>, String> {
    let mut order = Vec::new();
    let mut visited = std::collections::HashSet::new();
    let mut queue = std::collections::VecDeque::new();

    queue.push_back(target.to_string());

    while let Some(name) = queue.pop_front() {
        if visited.contains(&name) {
            continue;
        }
        visited.insert(name.clone());

        if let Some(spec) = specs.get(&name) {
            for req in &spec.requires {
                if req.kind == "capsule" && !visited.contains(&req.name) {
                    queue.push_back(req.name.clone());
                }
            }
        }

        order.push(name);
    }

    // Reverse: dependencies before dependents
    order.reverse();
    Ok(order)
}

// ---------------------------------------------------------------------------
// Command dispatch — CLI command forwarding from runtime
// ---------------------------------------------------------------------------

/// CLI command forwarded from runtime to shell for orchestration.
#[derive(serde::Serialize, Deserialize, Debug)]
#[serde(tag = "command")]
enum CommandRequest {
    #[serde(rename = "chat")]
    Chat {
        #[serde(default)]
        nick: Option<String>,
        #[serde(default)]
        connect: Option<String>,
        #[serde(default)]
        standalone: bool,
        #[serde(default)]
        no_history: bool,
        #[serde(default)]
        no_sync: bool,
        #[serde(default)]
        history_limit: Option<usize>,
    },

    #[serde(rename = "agent")]
    Agent {
        #[serde(default)]
        nick: Option<String>,
        #[serde(default)]
        channel: String,
        #[serde(default)]
        backend: String,
        #[serde(default)]
        connect: Option<String>,
        #[serde(default)]
        respond_all: bool,
    },

    #[serde(rename = "gateway")]
    Gateway {
        #[serde(default)]
        addr: String,
        #[serde(default)]
        public: bool,
        #[serde(default)]
        cache_dir: Option<String>,
    },

    #[serde(rename = "share")]
    Share {
        path: String,
        #[serde(default)]
        channel: Option<String>,
        #[serde(default)]
        no_attest: bool,
        #[serde(default)]
        no_head: bool,
    },

    #[serde(rename = "open")]
    Open {
        uri: String,
        #[serde(default)]
        browser: bool,
        #[serde(default)]
        port: Option<u16>,
    },

    #[serde(rename = "capsule")]
    Capsule {
        name: String,
        #[serde(default)]
        config: serde_json::Value,
        #[serde(default)]
        lifecycle: Option<String>,
        #[serde(default)]
        interactive: bool,
    },
}

/// Response from shell to runtime after command orchestration.
#[cfg(test)]
#[derive(serde::Serialize, Deserialize, Debug)]
struct CommandResponse {
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    handles: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[cfg(test)]
impl CommandResponse {
    fn ok(handles: Vec<String>) -> Self {
        Self {
            status: "ok".into(),
            handles: Some(handles),
            error: None,
        }
    }

    fn err(msg: impl Into<String>) -> Self {
        Self {
            status: "error".into(),
            handles: None,
            error: Some(msg.into()),
        }
    }
}

// ---------------------------------------------------------------------------
// Orchestrator — resolves deps and drives capsule lifecycle via supervisor
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandLifecycle {
    Interactive,
    OneShot,
    Daemon,
}

struct CommandSpec {
    target: String,
    lifecycle: CommandLifecycle,
    target_config: serde_json::Value,
    interactive_target: bool,
}

fn parse_lifecycle(raw: &str) -> Result<CommandLifecycle, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "interactive" => Ok(CommandLifecycle::Interactive),
        "oneshot" | "one-shot" | "one_shot" => Ok(CommandLifecycle::OneShot),
        "daemon" => Ok(CommandLifecycle::Daemon),
        other => Err(format!(
            "invalid lifecycle '{}'; expected interactive|oneshot|daemon",
            other
        )),
    }
}

fn command_spec(cmd: &CommandRequest) -> Result<CommandSpec, String> {
    let spec = match cmd {
        CommandRequest::Chat {
            nick,
            connect,
            standalone,
            no_history,
            no_sync,
            history_limit,
        } => CommandSpec {
            target: "chat".into(),
            lifecycle: CommandLifecycle::Interactive,
            interactive_target: true,
            target_config: serde_json::json!({
                "nick": nick,
                "connect": connect,
                "standalone": standalone,
                "no_history": no_history,
                "no_sync": no_sync,
                "history_limit": history_limit,
            }),
        },
        CommandRequest::Agent {
            nick,
            channel,
            backend,
            connect,
            respond_all,
        } => CommandSpec {
            target: "agent".into(),
            lifecycle: CommandLifecycle::Interactive,
            interactive_target: false,
            target_config: serde_json::json!({
                "nick": nick,
                "channel": channel,
                "backend": backend,
                "connect": connect,
                "respond_all": respond_all,
            }),
        },
        CommandRequest::Gateway {
            addr,
            public,
            cache_dir,
        } => CommandSpec {
            target: "gateway".into(),
            lifecycle: CommandLifecycle::Daemon,
            interactive_target: false,
            target_config: serde_json::json!({
                "addr": addr,
                "public": public,
                "cache_dir": cache_dir,
            }),
        },
        CommandRequest::Share {
            path,
            channel,
            no_attest,
            no_head,
        } => CommandSpec {
            target: "share".into(),
            lifecycle: CommandLifecycle::OneShot,
            interactive_target: false,
            target_config: serde_json::json!({
                "path": path,
                "channel": channel,
                "no_attest": no_attest,
                "no_head": no_head,
            }),
        },
        CommandRequest::Open { uri, browser, port } => CommandSpec {
            target: "open".into(),
            lifecycle: CommandLifecycle::OneShot,
            interactive_target: false,
            target_config: serde_json::json!({
                "uri": uri,
                "browser": browser,
                "port": port,
            }),
        },
        CommandRequest::Capsule {
            name,
            config,
            lifecycle,
            interactive,
        } => CommandSpec {
            target: name.clone(),
            lifecycle: match lifecycle.as_deref() {
                Some(v) => parse_lifecycle(v)?,
                None => CommandLifecycle::OneShot,
            },
            interactive_target: *interactive,
            target_config: config.clone(),
        },
    };
    Ok(spec)
}

/// Build capsule-specific config. This keeps provider wiring deterministic:
/// - llama-provider uses a fixed guest port
/// - ai-provider points local backend at host-forwarded llama endpoint
/// - target app capsule receives the CLI payload
fn config_for_capsule(
    cmd: &CommandRequest,
    spec: &CommandSpec,
    capsule: &str,
    is_target: bool,
) -> serde_json::Value {
    let mut config = serde_json::Map::new();

    if let CommandRequest::Agent { backend, .. } = cmd {
        if backend == "local" {
            if capsule == "llama-provider" {
                config.insert("extra".into(), serde_json::json!({ "port": 11434 }));
            } else if capsule == "ai-provider" {
                config.insert(
                    "extra".into(),
                    serde_json::json!({
                        "local_url": "http://172.16.0.1:11434/v1/chat/completions"
                    }),
                );
            }
        }
    }

    if is_target {
        if spec.interactive_target {
            config.insert("_elastos_interactive".into(), serde_json::json!(true));
        }
        if let serde_json::Value::Object(target_cfg) = spec.target_config.clone() {
            for (k, v) in target_cfg {
                config.insert(k, v);
            }
        }
    }

    serde_json::Value::Object(config)
}

/// Ask runtime supervisor for a manifest-driven transitive launch plan.
fn resolve_plan(
    api: &str,
    token: &str,
    target: &str,
) -> Result<(Vec<String>, Vec<String>), String> {
    let body = serde_json::json!({ "target": target });
    let resp = supervisor_post(api, token, "/api/supervisor/resolve-plan", &body)?;

    let capsules = resp
        .get("capsules")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "resolve-plan response missing 'capsules'".to_string())?
        .iter()
        .map(|v| {
            v.as_str()
                .ok_or_else(|| "resolve-plan capsules entry must be string".to_string())
                .map(|s| s.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;

    let externals = resp
        .get("externals")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "resolve-plan response missing 'externals'".to_string())?
        .iter()
        .map(|v| {
            v.as_str()
                .ok_or_else(|| "resolve-plan externals entry must be string".to_string())
                .map(|s| s.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok((capsules, externals))
}

// ---------------------------------------------------------------------------
// Command dispatch — forwards CLI commands to supervisor via HTTP
// ---------------------------------------------------------------------------

/// POST JSON to a supervisor endpoint. Returns parsed response or error string.
fn supervisor_post(
    api: &str,
    token: &str,
    endpoint: &str,
    body: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let url = format!("{}{}", api, endpoint);
    match agent()
        .post(&url)
        .set("Authorization", &format!("Bearer {}", token))
        .set("Content-Type", "application/json")
        .send_string(&body.to_string())
    {
        Ok(resp) => resp
            .into_json::<serde_json::Value>()
            .map_err(|e| format!("parse error: {}", e)),
        Err(ureq::Error::Status(code, resp)) => {
            let msg = resp.into_string().unwrap_or_default();
            Err(format!("HTTP {}: {}", code, msg))
        }
        Err(e) => Err(format!("request failed: {}", e)),
    }
}

fn stop_handles(api: &str, token: &str, handles: &[String]) {
    for handle in handles.iter().rev() {
        let stop_body = serde_json::json!({ "handle": handle });
        let _ = supervisor_post(api, token, "/api/supervisor/stop-capsule", &stop_body);
    }
}

fn merge_unique(dst: &mut Vec<String>, src: Vec<String>) {
    for item in src {
        if !dst.contains(&item) {
            dst.push(item);
        }
    }
}

/// Gateway is runtime-owned (HTTP server in host runtime) with tunnel-provider as public edge.
/// This path explicitly orchestrates:
///   1) ipfs-provider (for content fetches)
///   2) runtime gateway server startup
///   3) optional tunnel-provider public URL
fn dispatch_gateway(
    api: &str,
    token: &str,
    addr: &str,
    public: bool,
    cache_dir: Option<String>,
) -> Result<(), String> {
    let listen_addr = if addr.trim().is_empty() {
        "127.0.0.1:8090".to_string()
    } else {
        addr.trim().to_string()
    };

    let (ipfs_capsules, ipfs_externals) = resolve_plan(api, token, "ipfs-provider")
        .map_err(|e| format!("failed to resolve ipfs-provider launch plan: {}", e))?;

    let (tunnel_capsules, tunnel_externals) = if public {
        resolve_plan(api, token, "tunnel-provider")
            .map_err(|e| format!("failed to resolve tunnel-provider launch plan: {}", e))?
    } else {
        (Vec::new(), Vec::new())
    };

    let mut externals = Vec::new();
    merge_unique(&mut externals, ipfs_externals);
    merge_unique(&mut externals, tunnel_externals);

    let mut capsules = Vec::new();
    merge_unique(&mut capsules, ipfs_capsules);
    merge_unique(&mut capsules, tunnel_capsules);

    for ext in &externals {
        eprintln!("shell: ensuring external '{}'...", ext);
        let body = serde_json::json!({ "name": ext });
        supervisor_post(api, token, "/api/supervisor/ensure-external", &body)
            .map_err(|e| format!("failed to ensure external '{}': {}", ext, e))?;
    }

    for cap in &capsules {
        eprintln!("shell: ensuring capsule '{}'...", cap);
        let body = serde_json::json!({ "name": cap });
        supervisor_post(api, token, "/api/supervisor/ensure-capsule", &body)
            .map_err(|e| format!("failed to ensure capsule '{}': {}", cap, e))?;
    }

    let mut handles: Vec<String> = Vec::new();
    let mut tunnel_handle: Option<String> = None;
    let mut ipfs_handle: Option<String> = None;

    for cap in &capsules {
        eprintln!("shell: launching capsule '{}'...", cap);
        let body = serde_json::json!({
            "name": cap,
            "config": serde_json::json!({}),
        });
        let resp = match supervisor_post(api, token, "/api/supervisor/launch-capsule", &body) {
            Ok(v) => v,
            Err(e) => {
                stop_handles(api, token, &handles);
                return Err(format!("failed to launch capsule '{}': {}", cap, e));
            }
        };
        let handle = resp
            .get("handle")
            .and_then(|h| h.as_str())
            .ok_or_else(|| format!("launch response missing handle for '{}'", cap))?
            .to_string();
        if cap == "tunnel-provider" {
            tunnel_handle = Some(handle.clone());
        } else if cap == "ipfs-provider" {
            ipfs_handle = Some(handle.clone());
        }
        handles.push(handle);
    }

    let mut start_body = serde_json::json!({ "addr": listen_addr });
    if let Some(ref cache) = cache_dir {
        if !cache.trim().is_empty() {
            start_body["cache_dir"] = serde_json::Value::String(cache.clone());
        }
    }
    let gateway_resp =
        match supervisor_post(api, token, "/api/supervisor/start-gateway", &start_body) {
            Ok(v) => v,
            Err(e) => {
                stop_handles(api, token, &handles);
                return Err(format!("failed to start runtime gateway: {}", e));
            }
        };

    let effective_addr = gateway_resp
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or("127.0.0.1:8090");
    println!("Gateway: http://{}", effective_addr);

    if public {
        let target = format!("http://{}", effective_addr);
        let tunnel_resp = match supervisor_post(
            api,
            token,
            "/api/provider/tunnel/start",
            &serde_json::json!({
                "target": target,
            }),
        ) {
            Ok(v) => v,
            Err(e) => {
                stop_handles(api, token, &handles);
                return Err(format!("failed to start tunnel-provider: {}", e));
            }
        };

        if let Some(url) = tunnel_resp
            .get("data")
            .and_then(|d| d.get("url"))
            .and_then(|u| u.as_str())
        {
            println!("Public URL: {}", url);
            println!(
                "Installer URL template: {}/s/<installer-cid>/install.sh",
                url
            );
        } else {
            eprintln!(
                "shell: tunnel-provider response missing public URL: {}",
                tunnel_resp
            );
        }
    }

    let _ = (&tunnel_handle, &ipfs_handle); // kept for future health checks
    eprintln!("shell: gateway daemon running (press Ctrl+C in host runtime to stop)");

    // Gateway is a daemon command: keep shell VM alive as the orchestrator/watchdog.
    // Runtime stops this VM on Ctrl+C.
    loop {
        std::thread::sleep(std::time::Duration::from_secs(5));
        if supervisor_post(
            api,
            token,
            "/api/supervisor/start-gateway",
            &serde_json::json!({ "addr": effective_addr }),
        )
        .is_err()
        {
            stop_handles(api, token, &handles);
            return Err("gateway supervisor health check failed".to_string());
        }
    }
}

/// Dispatch a command forwarded from the runtime via ELASTOS_COMMAND.
///
/// Supervisor-driven orchestration path:
/// ensure externals → ensure capsules → launch in dependency order.
/// For interactive/daemon commands, wait on the target capsule handle.
fn dispatch_command(api: &str, token: &str, command_json: &str) {
    let cmd: CommandRequest = match serde_json::from_str(command_json) {
        Ok(cmd) => cmd,
        Err(e) => {
            eprintln!("shell: failed to parse ELASTOS_COMMAND: {}", e);
            eprintln!("shell: raw payload: {}", command_json);
            std::process::exit(1);
        }
    };

    eprintln!("shell: dispatching command: {:?}", cmd);

    if let CommandRequest::Gateway {
        addr,
        public,
        cache_dir,
    } = &cmd
    {
        if let Err(e) = dispatch_gateway(api, token, addr, *public, cache_dir.clone()) {
            eprintln!("shell: {}", e);
            std::process::exit(1);
        }
        return;
    }

    let spec = match command_spec(&cmd) {
        Ok(spec) => spec,
        Err(e) => {
            eprintln!("shell: invalid command configuration: {}", e);
            std::process::exit(1);
        }
    };

    let (capsules, externals) = match resolve_plan(api, token, &spec.target) {
        Ok(plan) => plan,
        Err(e) => {
            eprintln!(
                "shell: failed to resolve launch plan for '{}': {}",
                spec.target, e
            );
            eprintln!(
                "shell: command '{}' requires a published capsule named '{}' in components.json",
                match &cmd {
                    CommandRequest::Chat { .. } => "chat",
                    CommandRequest::Agent { .. } => "agent",
                    CommandRequest::Gateway { .. } => "gateway",
                    CommandRequest::Share { .. } => "share",
                    CommandRequest::Open { .. } => "open",
                    CommandRequest::Capsule { name, .. } => name,
                },
                spec.target
            );
            std::process::exit(1);
        }
    };
    let lifecycle = spec.lifecycle;

    // 1. Ensure external tools (kubo, cloudflared, etc.)
    for ext in &externals {
        eprintln!("shell: ensuring external '{}'...", ext);
        let body = serde_json::json!({ "name": ext });
        match supervisor_post(api, token, "/api/supervisor/ensure-external", &body) {
            Ok(_) => eprintln!("shell: external '{}' ready", ext),
            Err(e) => {
                eprintln!("shell: failed to ensure external '{}': {}", ext, e);
                std::process::exit(1);
            }
        }
    }

    // 2. Ensure capsule artifacts are downloaded and verified
    for cap in &capsules {
        eprintln!("shell: ensuring capsule '{}'...", cap);
        let body = serde_json::json!({ "name": cap });
        match supervisor_post(api, token, "/api/supervisor/ensure-capsule", &body) {
            Ok(_) => eprintln!("shell: capsule '{}' ready", cap),
            Err(e) => {
                eprintln!("shell: failed to ensure capsule '{}': {}", cap, e);
                std::process::exit(1);
            }
        }
    }

    // 3. Launch capsules in dependency order.
    //    Capsule-specific config handles provider wiring; target gets CLI config.
    let last_idx = capsules.len().saturating_sub(1);
    let mut handles: Vec<String> = Vec::new();
    for (i, cap) in capsules.iter().enumerate() {
        eprintln!("shell: launching capsule '{}'...", cap);
        let config = config_for_capsule(&cmd, &spec, cap.as_str(), i == last_idx);
        let body = serde_json::json!({
            "name": cap,
            "config": config,
        });
        match supervisor_post(api, token, "/api/supervisor/launch-capsule", &body) {
            Ok(resp) => {
                let handle = resp
                    .get("handle")
                    .and_then(|h| h.as_str())
                    .unwrap_or("unknown");
                eprintln!("shell: capsule '{}' launched (handle={})", cap, handle);
                handles.push(handle.to_string());
            }
            Err(e) => {
                eprintln!("shell: failed to launch capsule '{}': {}", cap, e);
                // Stop already-launched capsules on failure
                stop_handles(api, token, &handles);
                std::process::exit(1);
            }
        }
    }

    let target_handle = handles.last().cloned();

    // 4. For interactive and daemon commands, block on target lifecycle.
    //    The runtime now remains alive as long as the target capsule is running.
    //    While waiting, run a background approval loop so capability requests
    //    from launched capsules (e.g. chat requesting DID) get decided.
    if matches!(
        lifecycle,
        CommandLifecycle::Interactive | CommandLifecycle::Daemon
    ) {
        if let Some(target) = &target_handle {
            eprintln!(
                "shell: waiting for target capsule handle={} ({:?})...",
                target, lifecycle
            );

            // Background approval thread: poll and decide pending capabilities
            // using the same mode as the main shell loop.
            let dispatch_mode = resolve_mode(
                &std::env::var("ELASTOS_SHELL_MODE").unwrap_or_default(),
                std::io::stdin().is_terminal(),
            );
            let grant_api = api.to_string();
            let grant_token = token.to_string();
            let stop_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let stop_clone = stop_flag.clone();
            let grant_thread = std::thread::spawn(move || {
                let mut engine: Box<dyn DecisionEngine> = match dispatch_mode {
                    "auto" => Box::new(AutoGrantEngine),
                    "cli" => Box::new(CliPromptEngine::new()),
                    _ => Box::new(PolicyRulesEngine::load()),
                };
                let mut cooldown: HashMap<String, Instant> = HashMap::new();
                while !stop_clone.load(std::sync::atomic::Ordering::Relaxed) {
                    let pending = poll_pending(&grant_api, &grant_token);
                    let pending_ids: std::collections::HashSet<&str> =
                        pending.iter().map(|r| r.request_id.as_str()).collect();
                    cooldown.retain(|id, _| pending_ids.contains(id.as_str()));
                    for req in &pending {
                        if let Some(last) = cooldown.get(&req.request_id) {
                            if last.elapsed().as_secs() < DEFER_COOLDOWN_SECS {
                                continue;
                            }
                        }
                        let decision_req = DecisionRequest {
                            _request_id: req.request_id.clone(),
                            session_id: req.session_id.clone(),
                            resource: req.resource.clone(),
                            action: req.action.clone(),
                            _requested_at: req.requested_at,
                            _expires_at: req.expires_at,
                        };
                        let response = engine.decide(&decision_req);
                        if matches!(response.outcome, DecisionOutcome::Grant)
                            && post_grant(
                                &grant_api,
                                &grant_token,
                                &req.request_id,
                                &response.duration,
                                &response.rationale,
                            )
                        {
                            cooldown.insert(req.request_id.clone(), Instant::now());
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            });

            let wait_body = serde_json::json!({ "handle": target });
            if let Err(e) = supervisor_post(api, token, "/api/supervisor/wait-capsule", &wait_body)
            {
                eprintln!("shell: wait failed for target '{}': {}", target, e);
                stop_flag.store(true, std::sync::atomic::Ordering::Relaxed);
                let _ = grant_thread.join();
                stop_handles(api, token, &handles);
                std::process::exit(1);
            }

            stop_flag.store(true, std::sync::atomic::Ordering::Relaxed);
            let _ = grant_thread.join();
        }
    }

    // 5. Cleanup policy:
    // - Interactive/daemon: keep dependencies running so concurrently attached
    //   commands (e.g. chat + agent) share the same runtime/provider state.
    // - One-shot: stop dependencies after completion.
    if lifecycle == CommandLifecycle::OneShot && handles.len() > 1 {
        stop_handles(api, token, &handles[..handles.len() - 1]);
    }

    eprintln!(
        "shell: orchestration complete — {} capsules launched ({:?})",
        handles.len(),
        lifecycle
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_engine_grants_all() {
        let mut engine = AutoGrantEngine;
        let req = DecisionRequest {
            _request_id: "r1".into(),
            session_id: "s1".into(),
            resource: "localhost://Users/*".into(),
            action: "read".into(),
            _requested_at: 0,
            _expires_at: 0,
        };
        let resp = engine.decide(&req);
        assert_eq!(resp.outcome, DecisionOutcome::Grant);
        assert_eq!(resp.duration, "session");
    }

    #[test]
    fn policy_rules_engine_grants_matching_rule() {
        let mut engine = PolicyRulesEngine {
            allow_patterns: vec!["localhost://Users/*".into()],
        };
        let req = DecisionRequest {
            _request_id: "r1".into(),
            session_id: "s1".into(),
            resource: "localhost://Users/*".into(),
            action: "write".into(),
            _requested_at: 0,
            _expires_at: 0,
        };
        let resp = engine.decide(&req);
        assert_eq!(resp.outcome, DecisionOutcome::Grant);
        assert!(resp.rationale.contains("allowed by rule"));
    }

    #[test]
    fn policy_rules_engine_denies_missing_rule() {
        let mut engine = PolicyRulesEngine {
            allow_patterns: vec!["elastos://peer/*".into()],
        };
        let req = DecisionRequest {
            _request_id: "r1".into(),
            session_id: "s1".into(),
            resource: "localhost://Users/*".into(),
            action: "write".into(),
            _requested_at: 0,
            _expires_at: 0,
        };
        let resp = engine.decide(&req);
        assert_eq!(resp.outcome, DecisionOutcome::Deny);
        assert!(resp.rationale.contains("no allow rule"));
    }

    #[test]
    fn resolve_mode_default_secure() {
        // Default (no explicit mode): cli with TTY, agent without
        assert_eq!(resolve_mode("", true), "cli");
        assert_eq!(resolve_mode("", false), "agent");
        assert_eq!(resolve_mode("unknown", true), "cli");
        assert_eq!(resolve_mode("unknown", false), "agent");
    }

    #[test]
    fn resolve_mode_auto_explicit_only() {
        // "auto" must be explicitly requested
        assert_eq!(resolve_mode("auto", true), "auto");
        assert_eq!(resolve_mode("auto", false), "auto");
    }

    #[test]
    fn resolve_mode_cli_requires_tty() {
        assert_eq!(resolve_mode("cli", true), "cli");
        // cli without TTY falls to agent (not auto)
        assert_eq!(resolve_mode("cli", false), "agent");
    }

    #[test]
    fn resolve_mode_agent_ignores_tty() {
        assert_eq!(resolve_mode("agent", true), "agent");
        assert_eq!(resolve_mode("agent", false), "agent");
    }

    #[test]
    fn builtin_defaults_cover_core_providers() {
        let defaults = PolicyRulesEngine::builtin_defaults();
        assert!(defaults.contains(&"elastos://peer/*".to_string()));
        assert!(defaults.contains(&"elastos://did/*".to_string()));
        assert!(defaults.contains(&"localhost://Users/*".to_string()));
    }

    #[test]
    fn builtin_defaults_used_when_no_policy_file() {
        // PolicyRulesEngine with builtin defaults should grant peer access
        let engine = PolicyRulesEngine {
            allow_patterns: PolicyRulesEngine::builtin_defaults(),
        };
        assert!(engine.matches("elastos://peer/some-peer"));
        assert!(engine.matches("localhost://Users/self/Documents/file"));
        assert!(!engine.matches("https://external.com/api"));
    }

    #[test]
    fn resolve_deps_chat() {
        let mut specs = HashMap::new();
        specs.insert(
            "chat".into(),
            CapsuleSpec {
                _name: "chat".into(),
                requires: vec![CapsuleRequirement {
                    name: "did-provider".into(),
                    kind: "capsule".into(),
                }],
                _provides: None,
            },
        );
        specs.insert(
            "did-provider".into(),
            CapsuleSpec {
                _name: "did-provider".into(),
                requires: vec![],
                _provides: Some("elastos://did/*".into()),
            },
        );

        let order = resolve_dependencies("chat", &specs).unwrap();
        let did_pos = order.iter().position(|n| n == "did-provider").unwrap();
        let chat_pos = order.iter().position(|n| n == "chat").unwrap();
        assert!(did_pos < chat_pos, "did-provider must precede chat");
        assert_eq!(*order.last().unwrap(), "chat");
    }

    #[test]
    fn resolve_deps_no_deps() {
        let mut specs = HashMap::new();
        specs.insert(
            "hello".into(),
            CapsuleSpec {
                _name: "hello".into(),
                requires: vec![],
                _provides: None,
            },
        );
        let order = resolve_dependencies("hello", &specs).unwrap();
        assert_eq!(order, vec!["hello"]);
    }

    #[test]
    fn command_spec_for_chat() {
        let cmd = CommandRequest::Chat {
            nick: Some("alice".into()),
            connect: None,
            standalone: false,
            no_history: false,
            no_sync: false,
            history_limit: None,
        };
        let spec = command_spec(&cmd).unwrap();
        assert_eq!(spec.target, "chat");
        assert_eq!(spec.lifecycle, CommandLifecycle::Interactive);
        assert!(spec.interactive_target);
    }

    #[test]
    fn command_spec_for_agent() {
        let cmd = CommandRequest::Agent {
            nick: None,
            channel: "#general".into(),
            backend: "local".into(),
            connect: None,
            respond_all: false,
        };
        let spec = command_spec(&cmd).unwrap();
        assert_eq!(spec.target, "agent");
        assert_eq!(spec.lifecycle, CommandLifecycle::Interactive);
    }

    #[test]
    fn command_spec_for_gateway_public() {
        let cmd = CommandRequest::Gateway {
            addr: "0.0.0.0:8090".into(),
            public: true,
            cache_dir: None,
        };
        let spec = command_spec(&cmd).unwrap();
        assert_eq!(spec.target, "gateway");
        assert_eq!(spec.lifecycle, CommandLifecycle::Daemon);
    }

    #[test]
    fn command_spec_for_share() {
        let cmd = CommandRequest::Share {
            path: "README.md".into(),
            channel: Some("docs".into()),
            no_attest: true,
            no_head: false,
        };
        let spec = command_spec(&cmd).unwrap();
        assert_eq!(spec.target, "share");
        assert_eq!(spec.lifecycle, CommandLifecycle::OneShot);
        assert_eq!(spec.target_config["path"], "README.md");
        assert_eq!(spec.target_config["channel"], "docs");
        assert_eq!(spec.target_config["no_attest"], true);
    }

    #[test]
    fn command_spec_for_capsule_custom() {
        let cmd = CommandRequest::Capsule {
            name: "custom-worker".into(),
            config: serde_json::json!({"foo":"bar"}),
            lifecycle: Some("daemon".into()),
            interactive: true,
        };
        let spec = command_spec(&cmd).unwrap();
        assert_eq!(spec.target, "custom-worker");
        assert_eq!(spec.lifecycle, CommandLifecycle::Daemon);
        assert!(spec.interactive_target);
        assert_eq!(spec.target_config["foo"], "bar");
    }

    #[test]
    fn parse_lifecycle_variants() {
        assert_eq!(
            parse_lifecycle("interactive").unwrap(),
            CommandLifecycle::Interactive
        );
        assert_eq!(
            parse_lifecycle("oneshot").unwrap(),
            CommandLifecycle::OneShot
        );
        assert_eq!(
            parse_lifecycle("one-shot").unwrap(),
            CommandLifecycle::OneShot
        );
        assert_eq!(
            parse_lifecycle("one_shot").unwrap(),
            CommandLifecycle::OneShot
        );
        assert_eq!(parse_lifecycle("daemon").unwrap(), CommandLifecycle::Daemon);
        assert!(parse_lifecycle("invalid").is_err());
    }

    #[test]
    fn command_spec_lifecycle_modes() {
        let chat = CommandRequest::Chat {
            nick: None,
            connect: None,
            standalone: false,
            no_history: false,
            no_sync: false,
            history_limit: None,
        };
        let share = CommandRequest::Share {
            path: "README.md".into(),
            channel: None,
            no_attest: false,
            no_head: false,
        };
        let gateway = CommandRequest::Gateway {
            addr: "127.0.0.1:8090".into(),
            public: false,
            cache_dir: None,
        };
        assert_eq!(
            command_spec(&chat).unwrap().lifecycle,
            CommandLifecycle::Interactive
        );
        assert_eq!(
            command_spec(&share).unwrap().lifecycle,
            CommandLifecycle::OneShot
        );
        assert_eq!(
            command_spec(&gateway).unwrap().lifecycle,
            CommandLifecycle::Daemon
        );
    }

    #[test]
    fn command_request_serialization() {
        let cmd = CommandRequest::Chat {
            nick: Some("bob".into()),
            connect: None,
            standalone: false,
            no_history: false,
            no_sync: false,
            history_limit: None,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("\"command\":\"chat\""));
        assert!(json.contains("\"nick\":\"bob\""));

        let parsed: CommandRequest = serde_json::from_str(&json).unwrap();
        match parsed {
            CommandRequest::Chat { nick, .. } => assert_eq!(nick, Some("bob".into())),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn config_for_capsule_agent_local_wiring() {
        let cmd = CommandRequest::Agent {
            nick: Some("bot".into()),
            channel: "#general".into(),
            backend: "local".into(),
            connect: None,
            respond_all: false,
        };
        let spec = command_spec(&cmd).unwrap();

        let llama_cfg = config_for_capsule(&cmd, &spec, "llama-provider", false);
        assert_eq!(llama_cfg["extra"]["port"], 11434);

        let ai_cfg = config_for_capsule(&cmd, &spec, "ai-provider", false);
        assert_eq!(
            ai_cfg["extra"]["local_url"],
            "http://172.16.0.1:11434/v1/chat/completions"
        );
    }

    #[test]
    fn config_for_capsule_target_merges_cli_payload() {
        let cmd = CommandRequest::Agent {
            nick: Some("bot".into()),
            channel: "#general".into(),
            backend: "local".into(),
            connect: Some("ticket-1".into()),
            respond_all: true,
        };
        let spec = command_spec(&cmd).unwrap();
        let cfg = config_for_capsule(&cmd, &spec, "agent", true);
        assert_eq!(cfg["nick"], "bot");
        assert_eq!(cfg["channel"], "#general");
        assert_eq!(cfg["respond_all"], true);
        assert_eq!(cfg["backend"], "local");
    }

    #[test]
    fn config_for_capsule_chat_marks_interactive() {
        let cmd = CommandRequest::Chat {
            nick: Some("alice".into()),
            connect: None,
            standalone: false,
            no_history: false,
            no_sync: false,
            history_limit: None,
        };
        let spec = command_spec(&cmd).unwrap();
        let cfg = config_for_capsule(&cmd, &spec, "chat", true);
        assert_eq!(cfg["_elastos_interactive"], true);
        assert_eq!(cfg["nick"], "alice");
    }

    #[test]
    fn command_response_ok() {
        let resp = CommandResponse::ok(vec!["vm-chat-3".into(), "vm-did-4".into()]);
        assert_eq!(resp.status, "ok");
        assert_eq!(resp.handles.unwrap().len(), 2);
        assert!(resp.error.is_none());
    }

    #[test]
    fn command_response_err() {
        let resp = CommandResponse::err("capsule not found");
        assert_eq!(resp.status, "error");
        assert!(resp.handles.is_none());
        assert_eq!(resp.error.unwrap(), "capsule not found");
    }

    #[test]
    fn cooldown_prune_removes_resolved() {
        let mut cooldown: HashMap<String, Instant> = HashMap::new();
        cooldown.insert("r1".into(), Instant::now());
        cooldown.insert("r2".into(), Instant::now());

        // Simulate r1 still pending, r2 resolved
        let pending_ids: std::collections::HashSet<&str> = ["r1"].into_iter().collect();
        cooldown.retain(|id, _| pending_ids.contains(id.as_str()));

        assert!(cooldown.contains_key("r1"));
        assert!(!cooldown.contains_key("r2"));
    }
}
