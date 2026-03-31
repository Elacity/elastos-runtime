//! ElastOS Runtime CLI

mod agent_cmd;
mod capsule_cmd;
mod capsule_publish_cmd;
mod chat_cmd;
mod config_cmd;
mod gateway_entry;
mod identity_cmd;
mod init_cmd;
mod pc2_cmd;
mod publish;
mod release_cmd;
mod run_cmd;
mod security_cmd;
mod serve_cmd;
mod server_infra;
mod share_cmd;
mod shares_cmd;
mod shell_cmd;
mod site_cmd;
mod trust_cmd;
mod webspace_cmd;

use clap::{Parser, Subcommand};
use security_cmd::{EmergencyCommand, TlsCommand};
use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use trust_cmd::KeysCommand;

pub(crate) use chat_cmd::request_attached_capability;
use elastos_runtime::{provider, session};
#[cfg(test)]
pub(crate) use elastos_server::binaries::verify_component_binary_with_data_dir;
pub(crate) use elastos_server::binaries::{
    find_installed_provider_binary, resolve_verified_provider_binary, verify_component_binary,
};
use elastos_server::{api, runtime, setup};

use runtime::Runtime;

use elastos_compute::providers::WasmProvider;
use elastos_compute::ComputeProvider;
use elastos_storage::providers::LocalFSProvider;

use elastos_crosvm::{CrosvmConfig, CrosvmProvider};

const ELASTOS_VERSION: &str = env!("ELASTOS_VERSION");

#[cfg(unix)]
fn is_interactive_frontdoor_command(argv: &[String]) -> bool {
    if !(std::io::stdin().is_terminal() && std::io::stdout().is_terminal()) {
        return false;
    }

    matches!(
        argv.first().map(String::as_str),
        None | Some("pc2") | Some("chat") | Some("run")
    )
}

#[cfg(unix)]
fn should_isolate_process_group(argv: &[String]) -> bool {
    !is_interactive_frontdoor_command(argv)
}

#[derive(Parser)]
#[command(name = "elastos")]
#[command(about = "ElastOS - sovereign PC2 home and runtime\n\n\
    Run `elastos` with no subcommand to open your local PC2 home.\n\
    Use `elastos serve` for operator-runtime commands.\n\
    All resource access is capability-gated by the local control plane.")]
#[command(version = ELASTOS_VERSION)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Power-user path: run an arbitrary capsule from a local directory or IPFS CID
    Run {
        /// Path to capsule directory (or use --cid for IPFS)
        #[arg(required_unless_present = "cid")]
        path: Option<PathBuf>,

        /// IPFS CID of the capsule to run
        #[arg(long, conflicts_with = "path")]
        cid: Option<String>,

        /// Arguments to pass to the capsule (after --)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        capsule_args: Vec<String>,
    },

    /// Start the runtime daemon with HTTP API
    Serve {
        /// Address to bind to (use 0.0.0.0:3000 for LAN access)
        #[arg(short, long, default_value = "0.0.0.0:3000")]
        addr: String,

        /// Storage directory
        #[arg(long, default_value = "/tmp/elastos/storage")]
        storage_path: PathBuf,

        /// Capsule directory to serve (web capsule or MicroVM)
        #[arg(long, conflicts_with = "cid")]
        capsule: Option<PathBuf>,

        /// IPFS CID of capsule to serve
        #[arg(long, conflicts_with = "capsule")]
        cid: Option<String>,
    },

    /// Key management commands
    #[command(subcommand)]
    Keys(KeysCommand),

    /// Sign a capsule with a private key
    Sign {
        /// Path to capsule directory
        path: PathBuf,

        /// Path to private key file (hex-encoded)
        #[arg(short, long)]
        key: PathBuf,
    },

    /// Verify a capsule's signature or provenance attestation
    Verify {
        /// Path to capsule directory (capsule signature mode)
        #[arg(required_unless_present = "cid")]
        path: Option<PathBuf>,

        /// Path to public key file (capsule signature mode)
        #[arg(short = 'k', long, required_unless_present = "cid")]
        public_key: Option<PathBuf>,

        /// CID to verify provenance for (provenance mode)
        #[arg(long, conflicts_with_all = &["path", "public_key"])]
        cid: Option<String>,

        /// Provenance CID (if not in local catalog)
        #[arg(long, requires = "cid")]
        provenance: Option<String>,
    },

    /// Publish a capsule to IPFS
    Publish {
        /// Path to capsule directory
        path: PathBuf,
    },

    /// Publish a signed runtime release
    #[command(name = "publish-release")]
    PublishRelease {
        /// Semantic version for this release
        #[arg(long)]
        version: String,

        /// Release channel name
        #[arg(long, default_value = "stable")]
        channel: String,

        /// Publish profile name (used when --capsules is not set)
        #[arg(long, default_value = "demo")]
        profile: String,

        /// Skip rebuilding binaries
        #[arg(long)]
        skip_build: bool,

        /// Reuse existing capsule artifacts in artifacts/
        #[arg(long)]
        skip_rootfs: bool,

        /// Also publish for another architecture (for example: aarch64)
        #[arg(long)]
        cross: Option<String>,

        /// Capsule names to publish (comma-separated)
        #[arg(long, value_delimiter = ',')]
        capsules: Vec<String>,

        /// Override release signing key path
        #[arg(long)]
        key: Option<PathBuf>,

        /// Show the publish plan without building or uploading
        #[arg(long)]
        dry_run: bool,

        /// Validate publish prerequisites without building or uploading
        #[arg(long)]
        preflight_only: bool,

        /// Also start the public gateway/tunnel installer URL flow
        #[arg(long)]
        public_url: bool,

        /// Run the public gateway/tunnel step via sudo
        #[arg(long)]
        public_with_sudo: bool,

        /// Gateway listen address for the public URL flow
        #[arg(long, default_value = "127.0.0.1:8090")]
        gateway_addr: String,

        /// Seconds to wait for the public installer URL
        #[arg(long, default_value_t = 60)]
        public_timeout: u64,

        /// Override ipfs-provider binary path
        #[arg(long)]
        ipfs_provider_bin: Option<PathBuf>,

        /// Allow publishing without a stamped trusted-source bootstrap
        #[arg(long)]
        allow_no_bootstrap: bool,
    },

    /// Share a file or directory via IPFS (on a fresh install add `--with kubo --with ipfs-provider --with md-viewer`)
    Share {
        /// File or directory to share (e.g., README.md, docs/)
        path: PathBuf,

        /// Channel name for versioned sharing (default: derived from path)
        #[arg(long)]
        channel: Option<String>,

        /// Skip provenance attestation
        #[arg(long)]
        no_attest: bool,

        /// Skip signed channel head publication
        #[arg(long)]
        no_head: bool,

        /// Keep an immediate public link alive until Ctrl+C
        #[arg(long)]
        public: bool,

        /// Seconds to wait for the immediate public link
        #[arg(long, default_value_t = 60)]
        public_timeout: u64,
    },

    /// TLS certificate management
    #[command(subcommand)]
    Tls(TlsCommand),

    /// Launch native P2P chat
    Chat {
        /// Nickname to use
        #[arg(long)]
        nick: Option<String>,

        /// Connect to a peer via ticket for instant P2P
        #[arg(long)]
        connect: Option<String>,
    },

    /// Launch the PC2 home surface
    Pc2 {
        /// Print a plain CLI summary instead of the managed WASM dashboard
        #[arg(long)]
        status: bool,

        /// Emit machine-readable JSON (implies --status)
        #[arg(long)]
        json: bool,
    },

    /// Launch an AI agent that joins P2P chat and responds via LLM
    Agent {
        /// Agent persona name (defaults to a backend-specific persona like `codex`)
        #[arg(long)]
        nick: Option<String>,

        /// Gossip channel to join
        #[arg(long, default_value = "#general")]
        channel: String,

        /// AI backend to use (local, venice, codex)
        #[arg(long, default_value = "local")]
        backend: String,

        /// Respond to all messages, not just @mentions
        #[arg(long)]
        respond_all: bool,

        /// Connect to a peer via ticket (deterministic bootstrap)
        #[arg(long, short = 'c')]
        connect: Option<String>,
    },

    /// Manage runtime configuration
    #[command(subcommand)]
    Config(ConfigCommand),

    /// Show and manage the local DID-backed profile
    #[command(subcommand)]
    Identity(IdentityCommand),

    /// Emergency operations (key compromise response)
    #[command(subcommand)]
    Emergency(EmergencyCommand),

    /// Initialize a new capsule project
    Init {
        /// Name of the capsule to create
        name: String,

        /// Capsule type: wasm (default) or content (markdown viewer)
        #[arg(long, default_value = "wasm")]
        r#type: String,
    },

    /// Open a shared capsule by URI (on a fresh install add `--with kubo --with ipfs-provider --with md-viewer`)
    Open {
        /// elastos://<cid>, bare CID, https://gateway/ipfs/<cid>/, or localhost://MyWebSite
        uri: String,
        /// Open in browser automatically
        #[arg(long)]
        browser: bool,
        /// Port to serve on (default: find free port)
        #[arg(long)]
        port: Option<u16>,
    },

    /// Launch a packaged capsule by name
    Capsule {
        /// Capsule name from components registry (for example: chat, agent, did-provider)
        name: String,

        /// JSON config payload passed to the capsule (for example: '{"nick":"alice"}')
        #[arg(long)]
        config: Option<String>,

        /// Lifecycle mode: oneshot, interactive, or daemon
        #[arg(long, default_value = "oneshot")]
        lifecycle: String,

        /// Mark target as interactive (attach crosvm serial to stdio)
        #[arg(long)]
        interactive: bool,
    },

    /// Create a provenance attestation for an existing CID
    Attest {
        /// CID to attest (validates with cid crate)
        cid: String,

        /// Private key file (hex-encoded). Default: auto share signing key.
        #[arg(short, long)]
        key: Option<PathBuf>,

        /// Content digest override (sha256:...). Default: fetched from _share.json in CID.
        #[arg(long)]
        content_digest: Option<String>,
    },

    /// Manage shared content channels
    #[command(subcommand)]
    Shares(SharesCommand),

    /// Stage and serve the local browser-facing site from localhost://MyWebSite
    #[command(subcommand)]
    Site(SiteCommand),

    /// Inspect and resolve dynamic localhost://WebSpaces/<moniker>/... handles and typed mounted views
    #[command(subcommand)]
    Webspace(WebspaceCommand),

    /// Sign a payload from stdin with domain-separated Ed25519
    #[command(name = "sign-payload")]
    SignPayload {
        /// Domain separator (e.g. "elastos.release.v1")
        #[arg(long)]
        domain: String,
        /// Path to Ed25519 signing key (hex-encoded). If omitted, uses default share key.
        /// When --key is provided, the file MUST exist (no auto-generation).
        #[arg(long)]
        key: Option<std::path::PathBuf>,
    },

    /// Start the public ElastOS edge for MyWebSite, publisher objects, and CID content
    Gateway {
        /// Address to bind to (use 0.0.0.0 for public, or use --public)
        #[arg(short, long, default_value = "127.0.0.1:8090")]
        addr: String,
        /// Start a public tunnel via tunnel-provider capsule
        #[arg(long)]
        public: bool,
        /// Local cache directory for fetched content
        #[arg(long)]
        cache_dir: Option<PathBuf>,
        /// File to publish via IPFS at startup (prints full URL)
        #[arg(long)]
        publish: Option<PathBuf>,
    },

    /// Install external components (kubo, cloudflared, llama-server, models)
    Setup {
        /// Profile name (recommended: pc2, demo, irc; advanced: minimal, public-gateway, agent-local-ai, full)
        #[arg(long)]
        profile: Option<String>,

        /// Additional components to install (comma-separated)
        #[arg(long, value_delimiter = ',')]
        with: Vec<String>,

        /// Components to exclude (comma-separated)
        #[arg(long, value_delimiter = ',')]
        without: Vec<String>,

        /// List available components and profiles
        #[arg(long)]
        list: bool,
    },

    /// Manage trusted release sources
    #[command(subcommand)]
    Source(sources::SourceCommand),

    /// Show runtime version
    Version,

    /// Check for and install runtime updates from a trusted source
    Update {
        /// Only check for updates, don't install
        #[arg(long)]
        check: bool,

        /// HEAD CID (bypasses discovery — operator override)
        #[arg(long)]
        head_cid: Option<String>,

        /// Skip Carrier discovery and require an explicit transport override
        #[arg(long)]
        no_p2p: bool,

        /// Explicit transport override URL for HEAD discovery (repeatable, tries in order)
        #[arg(long = "gateway")]
        gateways: Vec<String>,

        /// Skip confirmation prompt and install immediately
        #[arg(long, alias = "no-confirm")]
        yes: bool,

        /// Rollback to a specific release head CID (forces install even if same version)
        #[arg(long)]
        rollback_to: Option<String>,
    },

    /// Check for and install runtime updates
    Upgrade {
        /// Only check for updates, don't install
        #[arg(long)]
        check: bool,

        /// HEAD CID (bypasses discovery — operator override)
        #[arg(long)]
        head_cid: Option<String>,

        /// Skip Carrier discovery and require an explicit transport override
        #[arg(long)]
        no_p2p: bool,

        /// Explicit transport override URL for HEAD discovery (repeatable, tries in order)
        #[arg(long = "gateway")]
        gateways: Vec<String>,

        /// Skip confirmation prompt and install immediately
        #[arg(long, alias = "no-confirm")]
        yes: bool,

        /// Rollback to a specific release head CID (forces install even if same version)
        #[arg(long)]
        rollback_to: Option<String>,
    },
}

#[derive(Subcommand)]
enum SharesCommand {
    /// List all share channels
    List,
    /// Show version history for a channel
    History {
        /// Channel name
        channel: String,
    },
    /// Remove a channel from local catalog (published content remains on IPFS)
    DeleteLocal {
        /// Channel name
        channel: String,
    },
    /// Mark a channel as archived (read-only, hidden from default listings)
    Archive {
        /// Channel name
        channel: String,
    },
    /// Restore an archived channel to active status
    Unarchive {
        /// Channel name
        channel: String,
    },
    /// Mark a channel as revoked with a reason (published content remains on IPFS)
    Revoke {
        /// Channel name
        channel: String,
        /// Reason for revocation
        #[arg(long)]
        reason: String,
    },
    /// Set the default author DID for all shares
    SetDid {
        /// DID to set (e.g. did:key:z6Mk...)
        did: String,
    },
    /// Fetch and verify the signed channel head for a channel
    Head {
        /// Channel name
        channel: String,
    },
}

#[derive(Subcommand)]
pub(crate) enum SiteCommand {
    /// Stage a local static site into localhost://MyWebSite
    Stage {
        /// Source directory containing the static site
        source: PathBuf,
    },
    /// Show the local root and filesystem path for localhost://MyWebSite
    Path,
    /// Publish the current site root as an immutable CID-backed bundle
    Publish {
        /// Rooted localhost target (defaults to localhost://MyWebSite)
        target: Option<String>,
        /// Optional friendly release name to store under Publisher state
        #[arg(long)]
        release: Option<String>,
    },
    /// List named site releases for a site root
    Releases {
        /// Rooted localhost target (defaults to localhost://MyWebSite)
        target: Option<String>,
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// List named release channels for a site root
    Channels {
        /// Rooted localhost target (defaults to localhost://MyWebSite)
        target: Option<String>,
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Show activation history for a site root
    History {
        /// Rooted localhost target (defaults to localhost://MyWebSite)
        target: Option<String>,
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Sign and activate the current site root into Edge site-head state
    Activate {
        /// Rooted localhost target (defaults to localhost://MyWebSite)
        target: Option<String>,
        /// Activate an existing named release instead of publishing the current working root
        #[arg(long)]
        release: Option<String>,
        /// Activate the named release currently promoted to this channel
        #[arg(long)]
        channel: Option<String>,
    },
    /// Roll back the active site head to a previous published bundle
    Rollback {
        /// Previously published site bundle CID prefix or named release. Omit to roll back to the previous activation.
        revision: Option<String>,
        /// Rooted localhost target (defaults to localhost://MyWebSite)
        target: Option<String>,
    },
    /// Bind a public domain to a rooted local site target through the gateway edge
    BindDomain {
        /// Public domain name, for example elastos.elacitylabs.com
        domain: String,
        /// Rooted localhost target (defaults to localhost://MyWebSite)
        target: Option<String>,
    },
    /// Promote a named release into an Edge release channel
    Promote {
        /// Channel name, for example live or staging
        channel: String,
        /// Existing named release to promote into that channel
        release: String,
        /// Rooted localhost target (defaults to localhost://MyWebSite)
        target: Option<String>,
    },
    /// Serve localhost://MyWebSite in local or ephemeral mode
    Serve {
        /// Gateway mode: local (bind directly) or ephemeral (cloudflared tunnel)
        #[arg(long, default_value = "local", value_parser = ["local", "ephemeral"])]
        mode: String,
        /// Address to bind to
        #[arg(short, long, default_value = "127.0.0.1:8081")]
        addr: String,
        /// Optional domain hint for local/static-IP mode
        #[arg(long)]
        domain: Option<String>,
        /// Open the served site in a browser
        #[arg(long)]
        browser: bool,
        /// Seconds to wait for an ephemeral public URL
        #[arg(long, default_value_t = 60)]
        public_timeout: u64,
    },
}

#[derive(Subcommand)]
pub(crate) enum WebspaceCommand {
    /// List the currently known WebSpace monikers or the typed children under a mounted handle
    List {
        /// Optional moniker or handle path (for example: Elastos or Elastos/peer)
        path: Option<String>,
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Resolve a WebSpace moniker or handle path into its local typed handle
    Resolve {
        /// Moniker or handle path, for example: Elastos or Elastos/content/<cid>
        target: String,
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Show current configuration
    Show,
    /// Set a configuration value
    Set {
        /// Key to set (e.g. "dev_mode", "enable_cache")
        key: String,
        /// Value to set
        value: String,
    },
}

#[derive(Subcommand)]
pub(crate) enum IdentityCommand {
    /// Show the current DID-backed local profile
    Show,
    /// Manage the profile nickname
    #[command(subcommand)]
    Nickname(IdentityNicknameCommand),
}

#[derive(Subcommand)]
pub(crate) enum IdentityNicknameCommand {
    /// Print the current nickname
    Get,
    /// Set the current nickname (prompts if omitted)
    Set {
        /// Nickname value
        value: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// Conditional tracing writer — suppresses stderr output during interactive VM
// ---------------------------------------------------------------------------

/// When true, tracing output is silently discarded (interactive TUI active).
static SUPPRESS_LOGGING: AtomicBool = AtomicBool::new(false);

pub(crate) fn set_logging_suppressed(suppress: bool) -> bool {
    SUPPRESS_LOGGING.swap(suppress, Ordering::Relaxed)
}

struct ConditionalStderr;

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for ConditionalStderr {
    type Writer = ConditionalStderrWriter;

    fn make_writer(&'a self) -> Self::Writer {
        ConditionalStderrWriter {
            suppress: SUPPRESS_LOGGING.load(Ordering::Relaxed),
        }
    }
}

struct ConditionalStderrWriter {
    suppress: bool,
}

impl std::io::Write for ConditionalStderrWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.suppress {
            Ok(buf.len())
        } else {
            std::io::stderr().write(buf)
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if self.suppress {
            Ok(())
        } else {
            std::io::stderr().flush()
        }
    }
}

// ---------------------------------------------------------------------------
// Host terminal raw mode — needed for interactive VM serial console
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        let argv: Vec<String> = std::env::args().skip(1).collect();
        if should_isolate_process_group(&argv) {
            // Non-interactive and operator flows still isolate their process group so
            // shutdown can reliably terminate spawned subprocesses. Interactive
            // front-door flows must retain the shell's foreground TTY ownership.
            unsafe {
                libc::setpgid(0, 0);
            }

            tokio::spawn(async move {
                let mut sigint =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
                        .expect("SIGINT handler");
                let mut sigterm =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                        .expect("SIGTERM handler");
                tokio::select! {
                    _ = sigint.recv() => {},
                    _ = sigterm.recv() => {},
                }
                eprintln!("\nShutting down...");
                // Kill entire process group (all child processes)
                unsafe {
                    libc::kill(0, libc::SIGTERM);
                }
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                // Force kill if still alive
                unsafe {
                    libc::kill(0, libc::SIGKILL);
                }
            });
        }
    }

    // Install rustls crypto provider (ring)
    let _ = rustls::crypto::ring::default_provider().install_default();

    // Initialize logging — uses ConditionalStderr so interactive VMs can suppress output.
    tracing_subscriber::fmt()
        .with_writer(ConditionalStderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("elastos=info".parse().expect("valid tracing directive")),
        )
        .init();

    let cli = Cli::parse();
    let command = cli.command.unwrap_or(Commands::Pc2 {
        status: false,
        json: false,
    });

    match command {
        Commands::Run {
            path,
            cid,
            capsule_args,
        } => {
            return run_cmd::run_capsule(path, cid, capsule_args).await;
        }

        Commands::Serve {
            addr,
            storage_path,
            capsule,
            cid,
        } => {
            return serve_cmd::run_serve(addr, storage_path, capsule, cid).await;
        }

        Commands::Keys(keys_cmd) => {
            trust_cmd::run_keys(keys_cmd)?;
        }

        Commands::Sign { path, key } => {
            trust_cmd::run_sign(path, key)?;
        }

        Commands::Verify {
            path,
            public_key,
            cid,
            provenance,
        } => {
            return trust_cmd::run_verify(path, public_key, cid, provenance).await;
        }

        Commands::Publish { path } => {
            return capsule_publish_cmd::run_publish(path).await;
        }

        Commands::Share {
            path,
            channel,
            no_attest,
            no_head,
            public,
            public_timeout,
        } => {
            return share_cmd::run_share(path, channel, no_attest, no_head, public, public_timeout)
                .await;
        }

        Commands::Chat { nick, connect } => {
            return chat_cmd::run_chat(nick, connect).await;
        }

        Commands::Pc2 { status, json } => {
            return pc2_cmd::run(status, json).await;
        }

        Commands::Agent {
            nick,
            channel,
            backend,
            respond_all,
            connect,
        } => {
            return agent_cmd::run_agent(nick, channel, backend, respond_all, connect).await;
        }

        Commands::Tls(tls_cmd) => {
            security_cmd::run_tls(tls_cmd)?;
        }

        Commands::Emergency(cmd) => {
            security_cmd::run_emergency(cmd)?;
        }

        Commands::Config(cmd) => {
            config_cmd::run_config(cmd)?;
        }

        Commands::Identity(cmd) => {
            return identity_cmd::run_identity(cmd).await;
        }

        Commands::Init { name, r#type } => {
            init_cmd::run_init(name, r#type)?;
        }

        Commands::Open { uri, browser, port } => {
            return share_cmd::run_open(uri, browser, port).await;
        }

        Commands::Attest {
            cid,
            key,
            content_digest,
        } => {
            return trust_cmd::run_attest(cid, key, content_digest).await;
        }

        Commands::Shares(cmd) => {
            return shares_cmd::run_shares(cmd).await;
        }

        Commands::Site(cmd) => {
            return site_cmd::run(cmd).await;
        }

        Commands::Webspace(cmd) => {
            return webspace_cmd::run(cmd).await;
        }

        Commands::SignPayload { domain, key } => {
            trust_cmd::run_sign_payload(domain, key)?;
        }

        Commands::PublishRelease {
            version,
            channel,
            profile,
            skip_build,
            skip_rootfs,
            cross,
            capsules,
            key,
            dry_run,
            preflight_only,
            public_url,
            public_with_sudo,
            gateway_addr,
            public_timeout,
            ipfs_provider_bin,
            allow_no_bootstrap,
        } => {
            release_cmd::run_publish_release(crate::publish::PublishReleaseOptions {
                version,
                channel,
                profile,
                skip_build,
                skip_rootfs,
                cross,
                capsules,
                key,
                dry_run,
                preflight_only,
                public_url,
                public_with_sudo,
                gateway_addr,
                public_timeout,
                ipfs_provider_bin,
                allow_no_bootstrap,
            })
            .await?;
        }

        // NOTE: Gateway bypasses the capsule-native (shell/supervisor) path.
        // It calls run_gateway_direct() directly. A later cleanup should move
        // this to a capsule-native gateway target.
        Commands::Gateway {
            addr,
            public,
            cache_dir,
            publish,
        } => {
            return gateway_entry::run_gateway(addr, public, cache_dir, publish).await;
        }

        Commands::Capsule {
            name,
            config,
            lifecycle,
            interactive,
        } => {
            return capsule_cmd::run_capsule(name, config, lifecycle, interactive).await;
        }

        Commands::Setup {
            profile,
            with,
            without,
            list,
        } => {
            setup::run(profile, with, without, list).await?;
        }

        Commands::Source(cmd) => {
            release_cmd::run_source(cmd)?;
        }

        Commands::Version => {
            release_cmd::run_version(ELASTOS_VERSION);
        }

        Commands::Update {
            check,
            head_cid,
            no_p2p,
            gateways,
            yes,
            rollback_to,
        }
        | Commands::Upgrade {
            check,
            head_cid,
            no_p2p,
            gateways,
            yes,
            rollback_to,
        } => {
            release_cmd::run_update_command(
                check,
                head_cid,
                no_p2p,
                gateways,
                yes,
                rollback_to,
                ELASTOS_VERSION,
            )
            .await?;
        }
    }

    Ok(())
}

/// Scaffold a new capsule project.
use elastos_server::sources::{self, default_data_dir};

use elastos_server::shares::{verify_channel_head, ChannelStatus, ShareMeta};
/// Print a welcome message on first run.
fn print_first_run_welcome(data_dir: &std::path::Path) {
    println!("Welcome to ElastOS!");
    println!();
    println!("  Data directory:  {}", data_dir.display());
    println!(
        "  Config file:     {}",
        data_dir.join("config.toml").display()
    );
    println!();
    println!("  Shell policy:    cli (interactive) with TTY, agent (rules) without");
    println!("  User path:       elastos chat auto-starts after setup");
    println!("  Operator path:   elastos serve, then elastos run / agent / capsule");
    println!();
}

async fn setup_server_infrastructure() -> anyhow::Result<server_infra::ServerInfrastructure> {
    server_infra::setup_server_infrastructure().await
}

// z32_encode removed — now using iroh::PublicKey::to_string() directly for node IDs.

use elastos_server::ipfs::IpfsBridge;

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct TunnelStatus {
    #[serde(default)]
    running: bool,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    last_log: Option<String>,
}

struct TunnelBridge {
    bridge: Arc<provider::ProviderBridge>,
}

impl TunnelBridge {
    fn new(bridge: Arc<provider::ProviderBridge>) -> Self {
        Self { bridge }
    }

    async fn start(&self, target: &str) -> anyhow::Result<TunnelStatus> {
        let resp = self
            .bridge
            .send_raw(&serde_json::json!({
                "op": "start",
                "target": target,
            }))
            .await
            .map_err(|e| anyhow::anyhow!("tunnel-provider bridge error: {}", e))?;
        parse_tunnel_status_response(resp, "start")
    }

    async fn status(&self) -> anyhow::Result<TunnelStatus> {
        let resp = self
            .bridge
            .send_raw(&serde_json::json!({ "op": "status" }))
            .await
            .map_err(|e| anyhow::anyhow!("tunnel-provider bridge error: {}", e))?;
        parse_tunnel_status_response(resp, "status")
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        self.bridge
            .shutdown()
            .await
            .map_err(|e| anyhow::anyhow!("tunnel-provider shutdown failed: {}", e))
    }
}

/// Create an IpfsBridge by spawning ipfs-provider capsule.
async fn get_ipfs_bridge() -> anyhow::Result<IpfsBridge> {
    let binary = find_installed_provider_binary("ipfs-provider").ok_or_else(|| {
        anyhow::anyhow!(
            "ipfs-provider not found. Run: elastos setup --with kubo --with ipfs-provider"
        )
    })?;
    verify_component_binary("ipfs-provider", &binary).map_err(|err| {
        anyhow::anyhow!(
            "ipfs-provider not ready. Run:\n\n  elastos setup --with kubo --with ipfs-provider\n\nDetails: {}",
            err
        )
    })?;
    let bridge = provider::ProviderBridge::spawn(&binary, Default::default())
        .await
        .map_err(|e| anyhow::anyhow!("Failed to spawn ipfs-provider: {}", e))?;
    Ok(IpfsBridge::new(Arc::new(bridge)))
}

async fn get_tunnel_bridge() -> anyhow::Result<TunnelBridge> {
    let binary = find_installed_provider_binary("tunnel-provider").ok_or_else(|| {
        anyhow::anyhow!(
            "tunnel-provider not found. Run: elastos setup --with tunnel-provider --with cloudflared"
        )
    })?;
    verify_component_binary("tunnel-provider", &binary)?;
    let bridge = provider::ProviderBridge::spawn(&binary, Default::default())
        .await
        .map_err(|e| anyhow::anyhow!("Failed to spawn tunnel-provider: {}", e))?;
    Ok(TunnelBridge::new(Arc::new(bridge)))
}

/// Serve a web capsule (or data capsule with viewer) with full runtime infrastructure.
///
/// Sets up sessions, shell, storage providers, and the API server.
/// Used by both `Commands::Run` (for data capsules) and `Commands::Serve` (for web capsules).
async fn serve_web_capsule(
    runtime: Runtime,
    capsule_dir: PathBuf,
    addr: &str,
    auto_open_browser: bool,
    public_preview_timeout_secs: Option<u64>,
) -> anyhow::Result<()> {
    // Read manifest if present (may have viewer field)
    let manifest_path = capsule_dir.join("capsule.json");
    let manifest: Option<elastos_common::CapsuleManifest> = if manifest_path.exists() {
        let data = tokio::fs::read_to_string(&manifest_path).await?;
        let m: elastos_common::CapsuleManifest = serde_json::from_str(&data)?;
        m.validate()
            .map_err(|e| anyhow::anyhow!("Invalid manifest: {}", e))?;
        Some(m)
    } else {
        None
    };

    let infra = setup_server_infrastructure().await?;
    let runtime = Arc::new(runtime);
    let docs_dir = std::env::current_dir().ok().and_then(|d| {
        let docs = d.join("..");
        if docs.join("ROADMAP.md").exists() {
            Some(docs)
        } else {
            None
        }
    });

    // Resolve viewer: if manifest has a viewer field, serve viewer as web root
    // and data capsule files at /capsule-data/
    let (serve_dir, data_dir) = match manifest.as_ref().and_then(|m| m.viewer.as_ref()) {
        Some(viewer_path) => {
            // Try relative path first (e.g. "../gba-emulator"), then search standard locations
            let relative_dir = capsule_dir.join(viewer_path);
            let viewer_dir = if relative_dir.exists() {
                relative_dir.canonicalize().unwrap_or(relative_dir)
            } else {
                elastos_server::ipfs::find_viewer_dir(viewer_path)?
            };
            tracing::info!("Viewer capsule: {}", viewer_dir.display());
            tracing::info!("Data capsule: {}", capsule_dir.display());
            (viewer_dir, Some(capsule_dir.clone()))
        }
        None => (capsule_dir.clone(), None),
    };

    // Create sessions
    let shell_session = infra
        .session_registry
        .create_session(session::SessionType::Shell, None)
        .await;
    let app_session = infra
        .session_registry
        .create_session(session::SessionType::Capsule, None)
        .await;

    // Set a stable owner so storage paths persist across server restarts.
    // Without this, session_user_id() hashes the random session ID, creating
    // a new storage directory each time the server starts.
    let stable_owner = manifest
        .as_ref()
        .map(|m| m.name.clone())
        .unwrap_or_else(|| "local".to_string());
    infra
        .session_registry
        .get_session_mut(&app_session.token, |s| {
            s.owner = Some(stable_owner);
        })
        .await;

    // Spawn shell capsule (decision engine for capability requests)
    let _shell_child = if let Some(shell_path) = find_installed_provider_binary("shell") {
        verify_component_binary("shell", &shell_path)?;
        let api_url = format!("http://{}", addr);
        let shell_mode = std::env::var("ELASTOS_SHELL_MODE").unwrap_or_else(|_| "auto".into());
        let stdin_cfg = if shell_mode == "cli" {
            std::process::Stdio::inherit()
        } else {
            std::process::Stdio::piped()
        };
        match tokio::process::Command::new(&shell_path)
            .env("ELASTOS_API", &api_url)
            .env("ELASTOS_TOKEN", &shell_session.token)
            .env("ELASTOS_SHELL_MODE", &shell_mode)
            .stdin(stdin_cfg)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(child) => {
                tracing::info!(
                    "Spawned shell capsule (PID {}, mode={})",
                    child.id().unwrap_or(0),
                    shell_mode
                );
                Some(child)
            }
            Err(e) => {
                tracing::warn!("Failed to spawn shell capsule: {}", e);
                None
            }
        }
    } else {
        tracing::info!("Shell capsule not found, skipping spawn");
        None
    };

    let capsule_name = manifest
        .as_ref()
        .map(|m| m.name.as_str())
        .unwrap_or("web-capsule");
    let has_viewer = manifest.as_ref().and_then(|m| m.viewer.as_ref()).is_some();

    // Bootstrap state only for data capsules with a viewer — the viewer's JS
    // needs the token, ROM entrypoint, and storage paths to auto-load
    let bootstrap_state = if has_viewer {
        manifest
            .as_ref()
            .map(|m| api::server::CapsuleBootstrapState {
                token: app_session.token.clone(),
                manifest: m.clone(),
            })
    } else {
        None
    };

    let local_url = format!("http://{}", addr);
    let mut public_tunnel = None;
    let public_url = match public_preview_timeout_secs {
        Some(timeout_secs) => match start_public_preview_tunnel(&local_url, timeout_secs).await {
            Ok((tunnel, public_url)) => {
                public_tunnel = Some(tunnel);
                Some(public_url)
            }
            Err(err) => {
                eprintln!(
                    "Note: public preview unavailable ({}). Continuing with local preview only.",
                    err
                );
                None
            }
        },
        None => None,
    };

    println!();
    println!("Serving: {}", capsule_name);
    println!("  Local preview: {}", local_url);
    if let Some(public_url) = public_url.as_ref() {
        println!("  Public preview: {}", public_url);
    }
    if public_url.is_some()
        && (std::env::var("SSH_CLIENT").is_ok() || std::env::var("SSH_TTY").is_ok())
    {
        println!("  (remote session — public preview avoids port forwarding)");
    } else if std::env::var("SSH_CLIENT").is_ok() || std::env::var("SSH_TTY").is_ok() {
        println!("  (remote session — use port forwarding or open on the device itself)");
    }
    println!();

    if auto_open_browser {
        open_browser(public_url.as_deref().unwrap_or(&local_url));
    }

    let server_result = api::server::start_server_with_sessions(api::server::ServerConfig {
        runtime,
        session_registry: infra.session_registry,
        capability_manager: infra.capability_manager,
        pending_store: infra.pending_store,
        namespace_store: Some(infra.namespace_store),
        provider_registry: Some(infra.provider_registry),
        audit_log: Some(infra.audit_log),
        identity_state: infra.identity_state,
        docs_dir,
        addr: addr.to_string(),
        capsule_dir: Some(serve_dir),
        data_dir,
        bootstrap_state,
        tls_config: None,
        supervisor: None,
        ready_tx: None,
        attach_secret: None,
    })
    .await;

    if let Some(tunnel) = public_tunnel {
        let _ = tunnel.shutdown().await;
    }

    server_result?;

    Ok(())
}

fn parse_tunnel_status_response(resp: serde_json::Value, op: &str) -> anyhow::Result<TunnelStatus> {
    if let Some(status) = resp.get("status").and_then(|s| s.as_str()) {
        if status == "error" {
            let code = resp
                .get("code")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown_error");
            let message = resp
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            anyhow::bail!("tunnel-provider {} failed [{}]: {}", op, code, message);
        }
    }

    let data = resp.get("data").cloned().unwrap_or(serde_json::Value::Null);
    serde_json::from_value(data)
        .map_err(|e| anyhow::anyhow!("Invalid tunnel-provider {} response: {}", op, e))
}

fn tunnel_public_share_url(base_url: &str, cid: &str) -> String {
    format!("{}/ipfs/{}/", base_url.trim_end_matches('/'), cid)
}

fn tunnel_status_detail(status: &TunnelStatus) -> String {
    status
        .last_log
        .as_deref()
        .map(|log| format!(" Last status: {}", log))
        .unwrap_or_default()
}

async fn start_public_preview_tunnel(
    target: &str,
    timeout_secs: u64,
) -> anyhow::Result<(TunnelBridge, String)> {
    let tunnel = get_tunnel_bridge().await?;
    let start_status = match tunnel.start(target).await {
        Ok(status) => status,
        Err(err) => {
            let _ = tunnel.shutdown().await;
            return Err(err);
        }
    };

    if let Some(url) = start_status.url.as_deref() {
        return Ok((tunnel, url.to_string()));
    }
    if !start_status.running {
        let detail = tunnel_status_detail(&start_status);
        let _ = tunnel.shutdown().await;
        anyhow::bail!(
            "tunnel-provider exited before publishing a public preview.{}",
            detail
        );
    }

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    loop {
        if std::time::Instant::now() >= deadline {
            let last = tunnel.status().await.unwrap_or_default();
            let detail = tunnel_status_detail(&last);
            let _ = tunnel.shutdown().await;
            anyhow::bail!(
                "timed out after {}s waiting for a public preview.{}",
                timeout_secs,
                detail
            );
        }

        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let status = match tunnel.status().await {
            Ok(status) => status,
            Err(err) => {
                let _ = tunnel.shutdown().await;
                return Err(err);
            }
        };

        if let Some(url) = status.url.as_deref() {
            return Ok((tunnel, url.to_string()));
        }
        if !status.running {
            let detail = tunnel_status_detail(&status);
            let _ = tunnel.shutdown().await;
            anyhow::bail!(
                "tunnel-provider exited before publishing a public preview.{}",
                detail
            );
        }
    }
}

async fn start_public_share_tunnel(
    ipfs: &IpfsBridge,
    cid: &str,
    timeout_secs: u64,
) -> anyhow::Result<(TunnelBridge, String)> {
    let ipfs_status = ipfs.status().await?;
    let target = ipfs_status
        .gateway_endpoint
        .ok_or_else(|| anyhow::anyhow!("ipfs-provider did not report a local gateway endpoint"))?;

    let tunnel = get_tunnel_bridge().await?;
    let start_status = match tunnel.start(&target).await {
        Ok(status) => status,
        Err(err) => {
            let _ = tunnel.shutdown().await;
            return Err(err);
        }
    };

    if let Some(url) = start_status.url.as_deref() {
        return Ok((tunnel, tunnel_public_share_url(url, cid)));
    }
    if !start_status.running {
        let detail = tunnel_status_detail(&start_status);
        let _ = tunnel.shutdown().await;
        anyhow::bail!(
            "tunnel-provider exited before publishing a public link.{}",
            detail
        );
    }

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    loop {
        if std::time::Instant::now() >= deadline {
            let last = tunnel.status().await.unwrap_or_default();
            let detail = tunnel_status_detail(&last);
            let _ = tunnel.shutdown().await;
            anyhow::bail!(
                "timed out after {}s waiting for a public link.{}",
                timeout_secs,
                detail
            );
        }

        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let status = match tunnel.status().await {
            Ok(status) => status,
            Err(err) => {
                let _ = tunnel.shutdown().await;
                return Err(err);
            }
        };

        if let Some(url) = status.url.as_deref() {
            return Ok((tunnel, tunnel_public_share_url(url, cid)));
        }
        if !status.running {
            let detail = tunnel_status_detail(&status);
            let _ = tunnel.shutdown().await;
            anyhow::bail!(
                "tunnel-provider exited before publishing a public link.{}",
                detail
            );
        }
    }
}

pub(crate) fn choose_local_open_addr(port: Option<u16>) -> anyhow::Result<String> {
    if let Some(port) = port {
        return Ok(format!("127.0.0.1:{}", port));
    }

    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(format!("127.0.0.1:{}", port))
}

async fn print_share_open_warnings(
    ipfs: &elastos_server::ipfs::IpfsBridge,
    catalog: &elastos_server::shares::ShareCatalog,
    cid: &str,
    meta: &ShareMeta,
) {
    let Some(channel) = catalog.channels.get(&meta.share_id) else {
        return;
    };

    let expected_did = channel.author_did.as_deref().or(meta.author_did.as_deref());

    if let Some(head_cid) = channel.head_cid.as_deref() {
        if let Ok(head_bytes) = ipfs.cat(head_cid).await {
            if let Ok(head) = verify_channel_head(&head_bytes) {
                let trusted = match expected_did {
                    Some(did) => head.payload.signer_did == did,
                    None => true,
                };
                if trusted {
                    match head.payload.status {
                        ChannelStatus::Archived => {
                            eprintln!("Note: channel '{}' is archived.", meta.share_id);
                        }
                        ChannelStatus::Revoked => {
                            eprintln!("WARNING: channel '{}' is revoked.", meta.share_id);
                        }
                        ChannelStatus::Active => {}
                    }
                    if head.payload.latest_cid != cid {
                        eprintln!(
                            "Note: newer version available: elastos://{}",
                            head.payload.latest_cid
                        );
                    }
                    return;
                }
            }
        }
    }

    match channel.status {
        ChannelStatus::Archived => {
            eprintln!("Note: channel '{}' is archived.", meta.share_id);
        }
        ChannelStatus::Revoked => {
            eprintln!("WARNING: channel '{}' is revoked.", meta.share_id);
        }
        ChannelStatus::Active => {}
    }
    if channel.latest_cid != cid {
        eprintln!(
            "Note: newer version available: elastos://{}",
            channel.latest_cid
        );
    }
}

/// Find a free local address for serving. Binds to port 0, reads the
/// assigned port, then drops the listener. Small TOCTOU window but
/// acceptable for localhost-only use.
fn find_free_local_addr() -> anyhow::Result<String> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| anyhow::anyhow!("Failed to find free port: {}", e))?;
    let addr = listener.local_addr()?;
    Ok(format!("127.0.0.1:{}", addr.port()))
}

/// Try to open a URL in the default browser.
pub(crate) fn open_browser(url: &str) {
    #[cfg(target_os = "linux")]
    {
        // Try WSL browser opener first, then xdg-open
        for cmd in &["wslview", "xdg-open"] {
            if std::process::Command::new(cmd)
                .arg(url)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .is_ok()
            {
                return;
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg(url)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
}

pub(crate) async fn create_runtime(
    storage_path: impl AsRef<std::path::Path>,
) -> anyhow::Result<Runtime> {
    let storage = Arc::new(LocalFSProvider::new(storage_path.as_ref().to_path_buf()).await?);

    // Build list of compute providers: WASM + crosvm
    let wasm_provider = Arc::new(WasmProvider::new());
    let base_provider: Arc<dyn ComputeProvider> = wasm_provider.clone();
    let mut compute_providers: Vec<Arc<dyn ComputeProvider>> = vec![base_provider];

    // Add crosvm provider if KVM is available
    if elastos_crosvm::is_supported() {
        match CrosvmProvider::new(CrosvmConfig::default()) {
            Ok(provider) => {
                if let Err(e) = provider.init().await {
                    tracing::warn!("Failed to initialize crosvm provider: {}", e);
                } else {
                    tracing::info!("crosvm provider enabled (KVM available)");
                    compute_providers.push(Arc::new(provider));
                }
            }
            Err(e) => {
                tracing::warn!("crosvm provider not available: {}", e);
            }
        }
    }

    Ok(Runtime::with_providers(
        storage,
        compute_providers,
        Some(wasm_provider),
    ))
}

// Tests for extracted modules are in their respective module files:
// crypto.rs, sources.rs, shares.rs, update.rs

#[cfg(test)]
mod tests {
    use super::verify_component_binary_with_data_dir;
    use sha2::Digest;
    use std::fs;

    #[test]
    fn verify_component_binary_with_data_dir_rejects_dev_path() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("dev-site-provider");
        fs::write(&path, b"dev-site-provider").unwrap();

        let err = verify_component_binary_with_data_dir(tmp.path(), "site-provider", &path)
            .unwrap_err()
            .to_string();
        assert!(err.contains("must resolve from an installed runtime path"));
    }

    #[test]
    fn verify_component_binary_with_data_dir_verifies_installed_agent_binary() {
        let data_dir = tempfile::tempdir().unwrap();
        let bin_dir = data_dir.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();

        let install_path = bin_dir.join("agent");
        let bytes = b"agent-binary";
        fs::write(&install_path, bytes).unwrap();

        fs::write(data_dir.path().join("components.json"), {
            let checksum = format!("sha256:{}", hex::encode(sha2::Sha256::digest(bytes)));
            format!(
                r#"{{
  "schema": "elastos.components/v1",
  "version": "0.1.0",
  "capsules": {{}},
  "external": {{
    "agent": {{
      "install_path": "bin/agent",
      "platforms": {{
        "linux-amd64": {{
          "checksum": "{}",
          "url": "https://example.invalid/agent"
        }}
      }}
    }}
  }},
  "profiles": {{}}
}}"#,
                checksum
            )
        })
        .unwrap();

        verify_component_binary_with_data_dir(data_dir.path(), "agent", &install_path).unwrap();
    }
}
