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

    println!("── 1. GRANT — a scoped, expiring, revocable mandate (not your keys) ──");
    let resp = http
        .post(format!("{api_url}/api/standing-grants/issue"))
        .header("Authorization", format!("Bearer {shell_token}"))
        .json(&serde_json::json!({
            "capsule": "vm-demo-agent",
            "resource": "elastos://pay/demo-vendor",
            "action": "write",
            "methods": ["pay.invoke"],
            "ttl_secs": 3600,
        }))
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(error_for(resp, "demo grant").await);
    }
    let out: serde_json::Value = resp.json().await?;
    let token_id = out
        .get("token_id")
        .and_then(|v| v.as_str())
        .context("no token id")?
        .to_string();
    println!("granted mandate {token_id}: vm-demo-agent may write elastos://pay/demo-vendor via pay.invoke, 1h TTL\n");

    println!("── 2. LIST — the mandate is LIVE on the operator surface ──");
    let list = fetch_mandate_list(&api_url, &shell_token).await?;
    print_mandate_list(&list);
    // The signer pin for step 5, obtained over the authenticated channel.
    let pinned_signer = list
        .get("signer_public_key_hex")
        .and_then(|v| v.as_str())
        .context("runtime did not report its audit signer (memory-only log?)")?
        .to_string();

    println!("\n── 3. REVOKE — the kill switch, durably attested before it mutates ──");
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

    println!("── 4. RECEIPT — the portable, set-bound proof of the whole mandate ──");
    let resp = http
        .get(format!("{api_url}/api/mandate/{token_id}/receipt"))
        .header("Authorization", format!("Bearer {shell_token}"))
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(error_for(resp, "demo receipt").await);
    }
    let receipt: elastos_runtime::primitives::MandateReceipt = resp.json().await?;
    println!("exported: {} records (grant … revoke), signed by {}\n", receipt.records.len(), receipt.signer_public_key_hex);

    println!("── 5. VERIFY — independently, pinned to the runtime key YOU trust ──");
    let verdict =
        elastos_runtime::primitives::verify_mandate_receipt(&receipt, Some(&pinned_signer));
    println!("  structurally valid: {}", verdict.structurally_valid);
    println!("  set binding:        {}", verdict.set_binding_ok);
    println!("  scope rule:         {}", verdict.scope_ok);
    println!("  AUTHENTICATED:      {}", verdict.authenticated);
    if !verdict.authenticated {
        bail!("demo receipt failed verification: {verdict:?}");
    }
    println!("\nThe loop is closed: authority granted, exercised scope visible, killed, and PROVEN —");
    println!("hand receipt + `elastos verify-receipt` to anyone; they need no runtime and no trust in this box.");
    Ok(())
}
