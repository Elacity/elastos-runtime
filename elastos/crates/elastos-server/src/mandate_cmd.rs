//! `elastos mandate` — the operator's mandate lifecycle: grant → revoke → prove.
//!
//! This is the Flint loop from the human side: hand an agent a SCOPED, EXPIRING, REVOCABLE
//! mandate instead of your keys (`grant`), kill it at any moment (`revoke`), and export the
//! portable, independently-verifiable proof of everything done under it (`receipt`, checked
//! off-box with `elastos verify-receipt`).
//!
//! The CLI holds NO authority of its own: every subcommand attaches to the RUNNING operator
//! runtime over the loopback control plane (the same attach-secret exchange the shell uses) and
//! calls the existing shell-scoped endpoints. The runtime remains the single writer of the audit
//! chain and the only holder of the signing key.

use anyhow::{bail, Context, Result};
use clap::Subcommand;

use crate::runtime_control;
use elastos_runtime::capability::IntentDeclarationV1;
use elastos_server::intent_executor::AUDIT_CHAIN_RESOURCE;
use elastos_server::sources::default_data_dir;

#[derive(Subcommand)]
pub(crate) enum MandateCommand {
    /// Grant an agent a scoped, expiring, revocable mandate (mints a real capability token)
    Grant {
        /// The acting capsule identity the mandate authorizes (e.g. "vm-agent")
        #[arg(long)]
        capsule: String,

        /// The resource the mandate covers (e.g. "elastos://pay/vendor")
        #[arg(long)]
        resource: String,

        /// The action: read | write | execute | delete | message | admin
        #[arg(long)]
        action: String,

        /// Affordance method the agent may invoke under this mandate (repeatable, at least one)
        #[arg(long = "method", required = true)]
        methods: Vec<String>,

        /// Time-to-live in seconds; omitted = until revoked
        #[arg(long)]
        ttl_secs: Option<u64>,

        /// Authorized agent's ed25519 public key (hex). When set, ONLY intents signed by this key
        /// may act under the mandate — the audit attribution is the real agent. Recommended.
        #[arg(long)]
        agent_key: Option<String>,
    },

    /// Revoke a mandate NOW — durably attested on the audit chain, then enforced fail-closed
    Revoke {
        /// The mandate's token id (printed by `mandate grant`)
        token_id: String,
    },

    /// Export the mandate's portable receipt (grant + every use/revoke) for off-box verification
    Receipt {
        /// The mandate's token id (printed by `mandate grant`)
        token_id: String,

        /// Write the receipt JSON here instead of stdout
        #[arg(short, long)]
        output: Option<std::path::PathBuf>,
    },

    /// List every standing mandate (live and revoked) with its scope and state
    List,

    /// Run the whole loop once against the live runtime: grant → list → revoke → receipt → verify
    Demo,
}

/// Attach to the RUNNING operator runtime and return (api_url, shell_token). The mandate
/// lifecycle is a control-plane operation, so it requires the runtime that enforces it.
async fn attach_shell() -> Result<(String, String)> {
    let data_dir = default_data_dir();
    let coords_path = runtime_control::runtime_coord_path(&data_dir);
    let coords = runtime_control::read_operator_runtime_coords(&coords_path)
        .await
        .ok_or_else(|| anyhow::anyhow!(runtime_control::OPERATOR_RUNTIME_REQUIRED_MESSAGE))?;
    if let Some(reason) = runtime_control::operator_runtime_staleness_reason(&coords).await? {
        bail!("{reason}");
    }
    let tokens = runtime_control::attach_to_runtime(&coords).await?;
    Ok((coords.api_url.clone(), tokens.shell_token))
}

fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .context("building HTTP client")
}

/// Surface a non-2xx response as the server's own error text (it is precise about why).
async fn error_for(resp: reqwest::Response, doing: &str) -> anyhow::Error {
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    anyhow::anyhow!("{doing} failed ({status}): {body}")
}

pub(crate) async fn run_mandate(cmd: MandateCommand) -> Result<()> {
    match cmd {
        MandateCommand::Grant {
            capsule,
            resource,
            action,
            methods,
            ttl_secs,
            agent_key,
        } => {
            let (api_url, shell_token) = attach_shell().await?;
            let resp = client()?
                .post(format!("{api_url}/api/standing-grants/issue"))
                .header("Authorization", format!("Bearer {shell_token}"))
                .json(&serde_json::json!({
                    "capsule": capsule,
                    "resource": resource,
                    "action": action,
                    "methods": methods,
                    "ttl_secs": ttl_secs,
                    "agent_pubkey": agent_key,
                }))
                .send()
                .await?;
            if !resp.status().is_success() {
                return Err(error_for(resp, "mandate grant").await);
            }
            let out: serde_json::Value = resp.json().await?;
            let token_id = out
                .get("token_id")
                .or_else(|| out.get("grant_id"))
                .and_then(|v| v.as_str())
                .context("runtime did not return a token id")?
                .to_string();
            println!("Mandate granted.");
            println!("  token id: {token_id}");
            println!("  capsule:  {capsule}");
            println!("  scope:    {action} on {resource}");
            println!("  methods:  {}", methods.join(", "));
            match ttl_secs {
                Some(secs) => println!("  expires:  in {secs}s"),
                None => println!("  expires:  never (until revoked)"),
            }
            match &agent_key {
                Some(k) => println!("  agent:    bound to {k}"),
                None => println!(
                    "  agent:    UNBOUND (any shell caller can act; pass --agent-key to bind)"
                ),
            }
            println!("\nRevoke:  elastos mandate revoke {token_id}");
            println!("Prove:   elastos mandate receipt {token_id} -o receipt.json");
            Ok(())
        }

        MandateCommand::Revoke { token_id } => {
            let (api_url, shell_token) = attach_shell().await?;
            let resp = client()?
                .post(format!("{api_url}/api/standing-grants/revoke"))
                .header("Authorization", format!("Bearer {shell_token}"))
                .json(&serde_json::json!({ "grant_id": token_id }))
                .send()
                .await?;
            if !resp.status().is_success() {
                return Err(error_for(resp, "mandate revoke").await);
            }
            let out: serde_json::Value = resp.json().await?;
            let envelope_was_live = out
                .get("revoked")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            // The durable CapabilityRevoke is attested by the runtime BEFORE this returns success
            // (emit-before-mutate), so reaching here means the revoke record is on the chain and
            // the token is dead in the runtime's persistent revocation store.
            println!("Token {token_id} revoked: signed CapabilityRevoke attested on the audit chain.");
            if envelope_was_live {
                println!("Its live standing mandate is killed; further dispatch is denied.");
            } else {
                // The kill switch's one job is "after this, the mandate is dead" — a mistyped
                // (but well-formed) id would revoke a token that never existed while the REAL
                // mandate stays live. Make that impossible to miss.
                eprintln!(
                    "WARNING: no LIVE standing mandate matched this id. If you expected to kill a \
                     live mandate, re-check the token id (`elastos mandate grant` printed it) — a \
                     mistyped id attests a revoke for a token that never existed while the real \
                     mandate STAYS LIVE. (This is expected if the mandate was already revoked or \
                     the token was granted outside the standing-mandate flow.)"
                );
            }
            println!("Prove it: elastos mandate receipt {token_id} -o receipt.json");
            Ok(())
        }

        MandateCommand::Receipt { token_id, output } => {
            let (api_url, shell_token) = attach_shell().await?;
            let resp = client()?
                .get(format!("{api_url}/api/mandate/{token_id}/receipt"))
                .header("Authorization", format!("Bearer {shell_token}"))
                .send()
                .await?;
            if !resp.status().is_success() {
                return Err(error_for(resp, "mandate receipt export").await);
            }
            let receipt: serde_json::Value = resp.json().await?;
            let pretty = serde_json::to_string_pretty(&receipt)?;
            let signer = receipt
                .get("signer_public_key_hex")
                .and_then(|v| v.as_str())
                .unwrap_or("<unknown>")
                .to_string();
            match output {
                Some(path) => {
                    std::fs::write(&path, &pretty)
                        .with_context(|| format!("writing {}", path.display()))?;
                    println!("Receipt written to {}", path.display());
                    // Deliberately NOT inlining the receipt's own embedded signer into the verify
                    // command: pinning the key a document carries about itself is circular — a
                    // wholesale forgery would "authenticate" against its own key. The pin must
                    // come from the verifier's out-of-band trust in the issuing runtime.
                    println!("  receipt's embedded signer (informational): {signer}");
                    println!(
                        "Verify off-box, pinning the issuer key YOU trust out-of-band:\n  \
                         elastos verify-receipt {} --signer <did:key-or-hex-you-trust>",
                        path.display()
                    );
                }
                None => println!("{pretty}"),
            }
            Ok(())
        }

        MandateCommand::List => {
            let (api_url, shell_token) = attach_shell().await?;
            let list = fetch_mandate_list(&api_url, &shell_token).await?;
            print_mandate_list(&list);
            Ok(())
        }

        MandateCommand::Demo => run_demo().await,
    }
}

async fn fetch_mandate_list(api_url: &str, shell_token: &str) -> Result<serde_json::Value> {
    let resp = client()?
        .get(format!("{api_url}/api/standing-grants"))
        .header("Authorization", format!("Bearer {shell_token}"))
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(error_for(resp, "mandate list").await);
    }
    Ok(resp.json().await?)
}

fn print_mandate_list(list: &serde_json::Value) {
    let mandates = list
        .get("mandates")
        .and_then(|m| m.as_array())
        .cloned()
        .unwrap_or_default();
    if mandates.is_empty() {
        println!("No standing mandates this runtime lifetime.");
        return;
    }
    println!("{} standing mandate(s):", mandates.len());
    for m in &mandates {
        let s = |k: &str| m.get(k).and_then(|v| v.as_str()).unwrap_or("?").to_string();
        let active = m.get("active").and_then(|v| v.as_bool()).unwrap_or(false);
        let revoked = m.get("revoked").and_then(|v| v.as_bool()).unwrap_or(false);
        let state = if active {
            "LIVE"
        } else if revoked {
            "REVOKED"
        } else {
            "EXPIRED"
        };
        let methods = m
            .get("methods")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", "))
            .unwrap_or_default();
        println!("\n  [{}] {}", state, s("token_id"));
        println!("    capsule: {}", s("capsule"));
        println!("    scope:   {} on {}", s("action"), s("resource"));
        println!("    methods: {methods}");
    }
}

/// The whole Flint loop, once, against the live runtime — nothing simulated, nothing fabricated:
/// a REAL mandate is granted (and left revoked at the end), every record lands on the REAL audit
/// chain, and the receipt is verified in-process with the signer pinned over the AUTHENTICATED
/// loopback control plane (the operator's trust in their own runtime — not the receipt vouching
/// for itself).
async fn run_demo() -> Result<()> {
    let (api_url, shell_token) = attach_shell().await?;
    let http = client()?;

    // The AGENT holds its OWN key. In production the agent generates and keeps this; the operator
    // only ever learns its PUBLIC half and binds the mandate to it — never the agent's keys, and
    // never the operator's. Here the demo stands in for the agent and generates an ephemeral key.
    let agent = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
    let agent_pub_hex = hex::encode(agent.verifying_key().to_bytes());

    println!(
        "(demo note: the ACTS below are real, but this process plays the agent — it generates the\n \
         agent key in-memory. In production the agent holds its own key; the operator binds only the\n \
         public half.)\n"
    );
    println!("── 1. GRANT — a scoped, expiring, revocable mandate bound to ONE agent key ──");
    let resp = http
        .post(format!("{api_url}/api/standing-grants/issue"))
        .header("Authorization", format!("Bearer {shell_token}"))
        .json(&serde_json::json!({
            "capsule": "vm-demo-agent",
            "resource": AUDIT_CHAIN_RESOURCE,
            "action": "read",
            "methods": ["runtime.audit_verify"],
            "ttl_secs": 3600,
            "agent_pubkey": agent_pub_hex,
        }))
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(error_for(resp, "demo grant").await);
    }
    // The grant SUCCEEDED (2xx) — from here a live mandate may exist. If we cannot read its token
    // id, we cannot target a cleanup revoke, so warn LOUDLY (the mandate is TTL-bounded and findable
    // via `mandate list`) rather than return silently as though nothing was granted.
    let token_id = resp
        .json::<serde_json::Value>()
        .await
        .ok()
        .and_then(|out| {
            out.get("token_id")
                .or_else(|| out.get("grant_id"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        });
    let token_id = match token_id {
        Some(t) => t,
        None => {
            eprintln!(
                "WARNING: the mandate was granted but its token id could not be read — a live \
                 mandate for vm-demo-agent may exist. Check `elastos mandate list` and revoke it."
            );
            bail!("grant response did not carry a token id");
        }
    };
    println!("granted mandate {token_id}: agent {} may runtime.audit_verify (read {AUDIT_CHAIN_RESOURCE}), 1h TTL\n", &agent_pub_hex[..16]);

    // From here a REAL 1h authority exists. If any later step fails, the mandate must not be
    // stranded live — attempt the revoke as cleanup before surfacing the error.
    match demo_after_grant(&http, &api_url, &shell_token, &token_id, &agent).await {
        Ok(()) => Ok(()),
        Err(e) => {
            eprintln!("demo step failed; revoking the demo mandate so no live authority is left behind…");
            let cleanup = http
                .post(format!("{api_url}/api/standing-grants/revoke"))
                .header("Authorization", format!("Bearer {shell_token}"))
                .json(&serde_json::json!({ "grant_id": token_id }))
                .send()
                .await;
            match cleanup {
                Ok(r) if r.status().is_success() => eprintln!("cleanup revoke succeeded."),
                _ => eprintln!(
                    "CLEANUP REVOKE FAILED — a live 1h demo mandate remains: run \
                     `elastos mandate revoke {token_id}`"
                ),
            }
            Err(e)
        }
    }
}

/// The agent signs an `audit_verify` intent under its own key and dispatches it. Returns the
/// dispatch outcome ("performed" / "denied" / …).
/// Returns `(outcome, reason)` — `reason` is the fail-closed denial reason (snake_case) when denied.
async fn agent_dispatch(
    http: &reqwest::Client,
    api_url: &str,
    shell_token: &str,
    agent: &ed25519_dalek::SigningKey,
    token_id: &str,
    intent_id: &str,
) -> Result<(String, Option<String>)> {
    let intent = IntentDeclarationV1::issue(
        agent,
        agent.verifying_key().to_bytes(),
        intent_id,
        "vm-demo-agent",
        "runtime.audit_verify",
        "", // no arguments
        AUDIT_CHAIN_RESOURCE,
        "read",
        token_id,
    );
    let resp = http
        .post(format!("{api_url}/api/standing-grants/dispatch"))
        .header("Authorization", format!("Bearer {shell_token}"))
        .json(&intent)
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(error_for(resp, "agent dispatch").await);
    }
    let out: serde_json::Value = resp.json().await?;
    let outcome = out
        .get("outcome")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let reason = out.get("reason").and_then(|v| v.as_str()).map(str::to_string);
    Ok((outcome, reason))
}

async fn demo_after_grant(
    http: &reqwest::Client,
    api_url: &str,
    shell_token: &str,
    token_id: &str,
    agent: &ed25519_dalek::SigningKey,
) -> Result<()> {
    println!("── 2. LIST — the mandate is LIVE on the operator surface ──");
    let list = fetch_mandate_list(api_url, shell_token).await?;
    print_mandate_list(&list);
    let pinned_signer = list
        .get("signer_public_key_hex")
        .and_then(|v| v.as_str())
        .context("runtime did not report its audit signer (memory-only log?)")?
        .to_string();

    // Intent ids are unique per demo RUN (suffixed with this run's fresh token id) so re-running the
    // demo against the same long-lived runtime does not collide with its per-lifetime replay guard.
    let act1_id = format!("demo-act-1-{token_id}");
    let act2_id = format!("demo-act-2-{token_id}");

    println!("\n── 3. ACT — the agent signs an intent and the runtime REALLY performs it ──");
    let (outcome, _) = agent_dispatch(http, api_url, shell_token, agent, token_id, &act1_id).await?;
    println!("agent dispatched runtime.audit_verify → {outcome}");
    if outcome != "performed" {
        bail!("expected the agent's authorized act to be performed, got {outcome:?}");
    }
    println!("(the runtime re-verified its own tamper-evident audit chain — a real, side-effect-free act)\n");

    println!("── 4. REVOKE — the kill switch, durably attested before it mutates ──");
    let resp = http
        .post(format!("{api_url}/api/standing-grants/revoke"))
        .header("Authorization", format!("Bearer {shell_token}"))
        .json(&serde_json::json!({ "grant_id": token_id }))
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(error_for(resp, "demo revoke").await);
    }
    println!("revoked {token_id}: signed CapabilityRevoke on the chain, envelope killed\n");

    println!("── 5. ACT AGAIN — the SAME agent, now denied (the kill switch has teeth) ──");
    let (after, reason) = agent_dispatch(http, api_url, shell_token, agent, token_id, &act2_id).await?;
    println!("agent re-dispatched after revoke → {after} ({})", reason.as_deref().unwrap_or("-"));
    // Must be denied SPECIFICALLY because the mandate was revoked — not some incidental denial.
    if after != "denied" || reason.as_deref() != Some("revoked") {
        bail!("expected the post-revoke act to be denied with reason=revoked, got {after:?}/{reason:?}");
    }
    println!();

    println!("── 6. RECEIPT — the portable, set-bound proof of the whole mandate ──");
    let resp = http
        .get(format!("{api_url}/api/mandate/{token_id}/receipt"))
        .header("Authorization", format!("Bearer {shell_token}"))
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(error_for(resp, "demo receipt").await);
    }
    let receipt: elastos_runtime::primitives::MandateReceipt = resp.json().await?;
    // Narrate the ACTUAL records the receipt carries (the token-keyed use records are best-effort,
    // so report what is really there rather than asserting a fixed trail).
    let trail: Vec<&str> = receipt
        .records
        .iter()
        .map(|r| match &r.event {
            elastos_runtime::primitives::AuditEvent::CapabilityGrant { .. } => "grant",
            elastos_runtime::primitives::AuditEvent::CapabilityUse { success: true, .. } => {
                "performed-read"
            }
            elastos_runtime::primitives::AuditEvent::CapabilityUse { success: false, .. } => {
                "denied-attempt"
            }
            elastos_runtime::primitives::AuditEvent::CapabilityRevoke { .. } => "revoke",
            _ => "other",
        })
        .collect();
    println!(
        "exported: {} records [{}], signed by {}\n",
        receipt.records.len(),
        trail.join(" → "),
        receipt.signer_public_key_hex
    );

    println!("── 7. VERIFY — the receipt against the pin from your runtime's control plane ──");
    // Honest scope: within one demo run the pin and the receipt come from the SAME runtime over
    // the SAME channel, so this demonstrates the pinning MECHANISM (and self-consistency), not
    // independent third-party verification — a counterparty runs verify-receipt on their own box
    // with a pin they obtained out-of-band.
    let verdict =
        elastos_runtime::primitives::verify_mandate_receipt(&receipt, Some(&pinned_signer));
    println!("  structurally valid: {}", verdict.structurally_valid);
    println!("  set binding:        {}", verdict.set_binding_ok);
    println!("  scope rule:         {}", verdict.scope_ok);
    println!("  pin matched:        {}", verdict.authenticated);
    println!("  (pin provenance: this runtime's control plane — self-asserted; a counterparty");
    println!("   pins the key they trust out-of-band and verifies on their own box)");
    if !verdict.authenticated {
        bail!("demo receipt failed verification: {verdict:?}");
    }
    println!("\nThe loop is closed: a scoped mandate GRANTED to one agent key, a real act PERFORMED");
    println!("under it, the kill switch REVOKED it, a post-revoke attempt DENIED, and the whole thing");
    println!("PROVEN — hand receipt + `elastos verify-receipt` to anyone; no runtime, no trust in this box.");
    Ok(())
}
