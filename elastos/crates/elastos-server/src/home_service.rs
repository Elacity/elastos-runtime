//! Home as a background service of the signed-in user: a systemd user unit on
//! Linux and a LaunchAgent on macOS. It runs as the user's own account and starts
//! `<data>/bin/elastos home --browser` with `ELASTOS_SERVICE=1`. When the service
//! cannot start, callers fall back to running Home in the terminal.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

pub const SERVICE_ENV: &str = "ELASTOS_SERVICE";
pub const SYSTEMD_UNIT: &str = "elastos-home.service";
pub const LAUNCHD_LABEL: &str = "org.elastos.home";
/// The controller's first start may repair support and waits up to 120 s for Home.
const READY_TIMEOUT: Duration = Duration::from_secs(150);
/// Covers the gateway's 15 s drain plus its children; the unit stop timeout matches.
const STOP_TIMEOUT: Duration = Duration::from_secs(60);

/// True inside the service: no browser, no terminal text.
pub fn service_mode() -> bool {
    std::env::var_os(SERVICE_ENV).as_deref() == Some(OsStr::new("1"))
}

/// While an install journal is pending, Home refuses to start and a restart cannot
/// change that. Under its service Home then exits cleanly, so the service manager
/// does not restart it in a loop; the installer, the updater or
/// `elastos home --browser` starts it again.
pub fn start_refused_by_pending_install() -> bool {
    std::env::current_exe().is_ok_and(|binary| {
        crate::install_transaction::InstallTransaction::has_pending_recovery(&binary)
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Linux,
    MacOs,
}

impl Platform {
    pub fn current() -> Option<Self> {
        if cfg!(target_os = "linux") {
            Some(Self::Linux)
        } else if cfg!(target_os = "macos") {
            Some(Self::MacOs)
        } else {
            None
        }
    }
}

/// What the service runs and where its definition lives.
#[derive(Clone, Debug)]
pub struct Spec {
    pub platform: Platform,
    pub home: PathBuf,
    pub data_dir: PathBuf,
    pub xdg_config_home: Option<PathBuf>,
    pub xdg_data_home: Option<PathBuf>,
    pub path: String,
    pub lang: Option<String>,
}

impl Spec {
    /// The signed-in user's service for this data directory.
    pub fn current(platform: Platform, data_dir: &Path) -> Result<Self> {
        let absolute = |key: &str| {
            std::env::var_os(key)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
        };
        let home = absolute("HOME")
            .or_else(dirs::home_dir)
            .context("home directory is unknown")?;
        let path = std::env::var("PATH")
            .ok()
            .map(|value| {
                value
                    .split(':')
                    .filter(|entry| entry.starts_with('/') && plain(entry))
                    .collect::<Vec<_>>()
                    .join(":")
            })
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin".into());
        Ok(Self {
            platform,
            home,
            data_dir: data_dir.to_path_buf(),
            xdg_config_home: absolute("XDG_CONFIG_HOME"),
            xdg_data_home: (platform == Platform::Linux)
                .then(|| absolute("XDG_DATA_HOME"))
                .flatten(),
            path,
            lang: std::env::var("LANG").ok().filter(|value| plain(value)),
        })
    }

    pub fn binary(&self) -> PathBuf {
        self.data_dir.join("bin").join("elastos")
    }

    pub fn definition(&self) -> PathBuf {
        match self.platform {
            Platform::Linux => self
                .xdg_config_home
                .clone()
                .unwrap_or_else(|| self.home.join(".config"))
                .join("systemd/user")
                .join(SYSTEMD_UNIT),
            Platform::MacOs => self
                .home
                .join("Library/LaunchAgents")
                .join(format!("{LAUNCHD_LABEL}.plist")),
        }
    }

    pub fn log(&self) -> PathBuf {
        self.data_dir.join("logs").join("home-service.log")
    }

    fn environment(&self) -> Vec<(&'static str, String)> {
        let mut values = vec![
            ("HOME", self.home.display().to_string()),
            ("PATH", self.path.clone()),
            (SERVICE_ENV, "1".into()),
        ];
        if let Some(lang) = &self.lang {
            values.push(("LANG", lang.clone()));
        }
        if let Some(data_home) = &self.xdg_data_home {
            values.push(("XDG_DATA_HOME", data_home.display().to_string()));
        }
        values
    }

    /// The unit file or property list text.
    pub fn render(&self) -> Result<String> {
        for path in [&self.home, &self.data_dir] {
            anyhow::ensure!(
                path.is_absolute() && path.to_str().is_some_and(plain),
                "service paths must be absolute plain text: {}",
                path.display()
            );
        }
        if let Some(path) = &self.xdg_data_home {
            anyhow::ensure!(
                path.to_str().is_some_and(plain),
                "XDG_DATA_HOME is not plain text"
            );
        }
        let binary = self.binary().display().to_string();
        let data = self.data_dir.display().to_string();
        Ok(match self.platform {
            Platform::Linux => {
                let environment = self
                    .environment()
                    .iter()
                    .map(|(key, value)| systemd_quote(&format!("{key}={value}")))
                    .collect::<Vec<_>>()
                    .join(" ");
                format!(
                    "# Written by the ElastOS installer. Home runs as your own account.\n\
                     [Unit]\n\
                     Description=ElastOS Home\n\
                     StartLimitIntervalSec=300\n\
                     StartLimitBurst=5\n\
                     \n\
                     [Service]\n\
                     Type=simple\n\
                     WorkingDirectory={}\n\
                     ExecStart={} home --browser\n\
                     Environment={environment}\n\
                     Restart=on-failure\n\
                     RestartSec=5\n\
                     TimeoutStopSec={}\n\
                     KillMode=mixed\n\
                     UMask=0077\n\
                     NoNewPrivileges=yes\n\
                     LockPersonality=yes\n\
                     RestrictRealtime=yes\n\
                     RestrictSUIDSGID=yes\n\
                     SystemCallArchitectures=native\n\
                     \n\
                     [Install]\n\
                     WantedBy=default.target\n",
                    data.replace('%', "%%"),
                    systemd_quote(&binary),
                    STOP_TIMEOUT.as_secs() - 15,
                )
            }
            Platform::MacOs => {
                let environment = self
                    .environment()
                    .iter()
                    .map(|(key, value)| {
                        format!(
                            "\t\t<key>{key}</key>\n\t\t<string>{}</string>\n",
                            xml_escape(value)
                        )
                    })
                    .collect::<String>();
                let log = xml_escape(&self.log().display().to_string());
                format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                     <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
                     <plist version=\"1.0\">\n\
                     <dict>\n\
                     \t<key>Label</key>\n\t<string>{LAUNCHD_LABEL}</string>\n\
                     \t<key>ProgramArguments</key>\n\t<array>\n\t\t<string>{}</string>\n\t\t<string>home</string>\n\t\t<string>--browser</string>\n\t</array>\n\
                     \t<key>WorkingDirectory</key>\n\t<string>{}</string>\n\
                     \t<key>EnvironmentVariables</key>\n\t<dict>\n{environment}\t</dict>\n\
                     \t<key>RunAtLoad</key>\n\t<true/>\n\
                     \t<key>KeepAlive</key>\n\t<dict>\n\t\t<key>SuccessfulExit</key>\n\t\t<false/>\n\t</dict>\n\
                     \t<key>ThrottleInterval</key>\n\t<integer>30</integer>\n\
                     \t<key>ExitTimeOut</key>\n\t<integer>{}</integer>\n\
                     \t<key>Umask</key>\n\t<integer>63</integer>\n\
                     \t<key>LimitLoadToSessionType</key>\n\t<string>Aqua</string>\n\
                     \t<key>StandardOutPath</key>\n\t<string>{log}</string>\n\
                     \t<key>StandardErrorPath</key>\n\t<string>{log}</string>\n\
                     </dict>\n\
                     </plist>\n",
                    xml_escape(&binary),
                    xml_escape(&data),
                    STOP_TIMEOUT.as_secs() - 15,
                )
            }
        })
    }
}

fn plain(value: &str) -> bool {
    !value.is_empty()
        && value.trim() == value
        && !value
            .chars()
            .any(|c| c.is_control() || matches!(c, '"' | '\\'))
}

fn systemd_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('%', "%%"))
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Runs the service manager's commands; tests replace it.
pub trait Manager {
    fn run(&self, program: &str, args: &[&OsStr]) -> std::io::Result<Output>;
}

pub struct SystemManager;

impl Manager for SystemManager {
    fn run(&self, program: &str, args: &[&OsStr]) -> std::io::Result<Output> {
        Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .output()
    }
}

fn run(manager: &dyn Manager, program: &str, args: &[&OsStr]) -> Result<Output> {
    manager
        .run(program, args)
        .with_context(|| format!("{program} is unavailable"))
}

fn checked(manager: &dyn Manager, program: &str, args: &[&OsStr]) -> Result<()> {
    let output = run(manager, program, args)?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        let detail = detail
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("");
        bail!(
            "{program} {} failed{}{}",
            args.first()
                .map(|arg| arg.to_string_lossy())
                .unwrap_or_default(),
            if detail.is_empty() { "" } else { ": " },
            detail.trim()
        );
    }
    Ok(())
}

fn gui_domain() -> String {
    format!("gui/{}", unsafe { libc::getuid() })
}

fn launchd_target() -> String {
    format!("{}/{LAUNCHD_LABEL}", gui_domain())
}

fn os(value: &str) -> OsString {
    OsString::from(value)
}

/// One user's Home service.
pub struct Service<'a> {
    pub spec: Spec,
    manager: &'a dyn Manager,
}

/// Unit state as the service manager reports it.
#[derive(Debug, PartialEq, Eq)]
pub enum State {
    Running,
    Stopped,
    /// The manager reports a failed start; the reason is for one line of output.
    Failed(String),
}

impl<'a> Service<'a> {
    pub fn new(spec: Spec, manager: &'a dyn Manager) -> Self {
        Self { spec, manager }
    }

    /// The definition exists and runs this data directory's Runtime; another
    /// installation's (or a test's) service is never stopped or started.
    pub fn installed(&self) -> bool {
        let Ok(text) = fs::read_to_string(self.spec.definition()) else {
            return false;
        };
        let binary = self.spec.binary().display().to_string();
        text.contains(&match self.spec.platform {
            Platform::Linux => format!("\nExecStart={} home --browser\n", systemd_quote(&binary)),
            Platform::MacOs => format!("<array>\n\t\t<string>{}</string>\n", xml_escape(&binary)),
        })
    }

    pub fn state(&self) -> Result<State> {
        Ok(self.observe()?.0)
    }

    /// The state and, on Linux, how often the manager restarted Home since it was
    /// last started by hand.
    fn observe(&self) -> Result<(State, u32)> {
        match self.spec.platform {
            Platform::Linux => {
                let output = run(
                    self.manager,
                    "systemctl",
                    &[
                        OsStr::new("--user"),
                        OsStr::new("show"),
                        OsStr::new("--property=ActiveState,SubState,NRestarts,Result"),
                        OsStr::new(SYSTEMD_UNIT),
                    ],
                )?;
                anyhow::ensure!(
                    output.status.success(),
                    "systemctl --user is unavailable: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                );
                let text = String::from_utf8_lossy(&output.stdout);
                let field = |name: &str| {
                    text.lines()
                        .find_map(|line| line.strip_prefix(name)?.strip_prefix('='))
                        .unwrap_or("")
                        .to_string()
                };
                let (active, sub, result) =
                    (field("ActiveState"), field("SubState"), field("Result"));
                let restarts: u32 = field("NRestarts").parse().unwrap_or(0);
                Ok((
                    match active.as_str() {
                        "active" | "activating" | "reloading" | "deactivating"
                            if sub != "auto-restart" =>
                        {
                            State::Running
                        }
                        "inactive" if result == "success" || result.is_empty() => State::Stopped,
                        _ => {
                            State::Failed(format!("the unit is {active} ({sub}, result {result})"))
                        }
                    },
                    restarts,
                ))
            }
            Platform::MacOs => {
                let output = run(
                    self.manager,
                    "launchctl",
                    &[OsStr::new("print"), &os(&launchd_target())],
                )?;
                if !output.status.success() {
                    return Ok((State::Stopped, 0));
                }
                let text = String::from_utf8_lossy(&output.stdout);
                let value = |name: &str| {
                    text.lines()
                        .find_map(|line| line.trim().strip_prefix(name)?.strip_prefix(" = "))
                        .map(str::trim)
                        .unwrap_or("")
                        .to_string()
                };
                if value("state") == "running" {
                    return Ok((State::Running, 0));
                }
                let exit = value("last exit code");
                Ok((
                    if exit.is_empty() || exit.starts_with("(never") {
                        // Loaded and about to start for the first time.
                        State::Running
                    } else if exit == "0" {
                        State::Stopped
                    } else {
                        State::Failed(format!("the agent exited with {exit}"))
                    },
                    0,
                ))
            }
        }
    }

    pub fn running(&self) -> bool {
        self.installed() && matches!(self.state(), Ok(State::Running))
    }

    fn write_definition(&self) -> Result<()> {
        let text = self.spec.render()?;
        let path = self.spec.definition();
        let parent = path.parent().context("service definition parent missing")?;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o755)
            .create(parent)?;
        if self.spec.platform == Platform::MacOs {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(self.spec.log().parent().unwrap())?;
        }
        let temporary = parent.join(format!(".{}.tmp", std::process::id()));
        let result = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o644)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&temporary)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
            fs::rename(&temporary, &path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        Ok(result?)
    }

    fn register(&self) -> Result<()> {
        let definition = self.spec.definition();
        match self.spec.platform {
            Platform::Linux => {
                let user = |args: &[&OsStr]| {
                    let mut all = vec![OsStr::new("--user")];
                    all.extend_from_slice(args);
                    checked(self.manager, "systemctl", &all)
                };
                user(&[OsStr::new("daemon-reload")])?;
                // A HOME other than the user manager's is outside its search path;
                // enabling by path links the unit first.
                if user(&[OsStr::new("enable"), OsStr::new(SYSTEMD_UNIT)]).is_err() {
                    user(&[OsStr::new("enable"), definition.as_os_str()])?;
                }
                user(&[OsStr::new("restart"), OsStr::new(SYSTEMD_UNIT)])
            }
            Platform::MacOs => {
                // A loaded agent keeps its old definition until it is booted out.
                let _ = run(
                    self.manager,
                    "launchctl",
                    &[OsStr::new("bootout"), &os(&launchd_target())],
                );
                self.wait_unloaded()?;
                checked(
                    self.manager,
                    "launchctl",
                    &[OsStr::new("enable"), &os(&launchd_target())],
                )?;
                checked(
                    self.manager,
                    "launchctl",
                    &[
                        OsStr::new("bootstrap"),
                        &os(&gui_domain()),
                        definition.as_os_str(),
                    ],
                )
            }
        }
    }

    fn wait_unloaded(&self) -> Result<()> {
        let deadline = Instant::now() + STOP_TIMEOUT;
        loop {
            let output = run(
                self.manager,
                "launchctl",
                &[OsStr::new("print"), &os(&launchd_target())],
            )?;
            if !output.status.success() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                bail!(
                    "the previous Home agent did not stop within {} seconds",
                    STOP_TIMEOUT.as_secs()
                );
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    /// Writes the definition, registers it with the user's service manager and
    /// starts Home. On any failure the definition is removed again.
    pub fn install(&self) -> Result<()> {
        let result = self.write_definition().and_then(|()| self.register());
        if let Err(error) = result {
            let _ = self.remove();
            return Err(error);
        }
        Ok(())
    }

    pub fn start(&self) -> Result<()> {
        match self.spec.platform {
            Platform::Linux => {
                // A failed start leaves a start-limit record that would refuse this one.
                let _ = run(
                    self.manager,
                    "systemctl",
                    &[
                        OsStr::new("--user"),
                        OsStr::new("reset-failed"),
                        OsStr::new(SYSTEMD_UNIT),
                    ],
                );
                checked(
                    self.manager,
                    "systemctl",
                    &[
                        OsStr::new("--user"),
                        OsStr::new("start"),
                        OsStr::new(SYSTEMD_UNIT),
                    ],
                )
            }
            Platform::MacOs => {
                let loaded = run(
                    self.manager,
                    "launchctl",
                    &[OsStr::new("print"), &os(&launchd_target())],
                )?
                .status
                .success();
                if loaded {
                    checked(
                        self.manager,
                        "launchctl",
                        &[OsStr::new("kickstart"), &os(&launchd_target())],
                    )
                } else {
                    checked(
                        self.manager,
                        "launchctl",
                        &[
                            OsStr::new("bootstrap"),
                            &os(&gui_domain()),
                            self.spec.definition().as_os_str(),
                        ],
                    )
                }
            }
        }
    }

    /// Ordered stop: the controller stops Home and its children before it exits.
    pub fn stop(&self) -> Result<()> {
        match self.spec.platform {
            Platform::Linux => checked(
                self.manager,
                "systemctl",
                &[
                    OsStr::new("--user"),
                    OsStr::new("stop"),
                    OsStr::new(SYSTEMD_UNIT),
                ],
            ),
            Platform::MacOs => {
                let _ = run(
                    self.manager,
                    "launchctl",
                    &[OsStr::new("bootout"), &os(&launchd_target())],
                )?;
                self.wait_unloaded()
            }
        }
    }

    /// Stops and unregisters the service, then deletes its definition.
    pub fn remove(&self) -> Result<()> {
        let definition = self.spec.definition();
        match self.spec.platform {
            Platform::Linux => {
                let _ = run(
                    self.manager,
                    "systemctl",
                    &[
                        OsStr::new("--user"),
                        OsStr::new("disable"),
                        OsStr::new("--now"),
                        OsStr::new(SYSTEMD_UNIT),
                    ],
                );
                remove_file(&definition)?;
                let _ = run(
                    self.manager,
                    "systemctl",
                    &[OsStr::new("--user"), OsStr::new("daemon-reload")],
                );
                let _ = run(
                    self.manager,
                    "systemctl",
                    &[
                        OsStr::new("--user"),
                        OsStr::new("reset-failed"),
                        OsStr::new(SYSTEMD_UNIT),
                    ],
                );
            }
            Platform::MacOs => {
                self.stop()?;
                remove_file(&definition)?;
            }
        }
        Ok(())
    }

    /// Linux user units stop at logout unless the user lingers. Returns one
    /// sentence for the user when lingering could not be turned on.
    pub fn ensure_linger(&self) -> Option<String> {
        if self.spec.platform != Platform::Linux {
            return None;
        }
        let uid = unsafe { libc::getuid() }.to_string();
        let lingering = run(
            self.manager,
            "loginctl",
            &[
                OsStr::new("show-user"),
                OsStr::new(&uid),
                OsStr::new("--property=Linger"),
                OsStr::new("--value"),
            ],
        )
        .ok()
        .is_some_and(|output| output.status.success() && output.stdout.trim_ascii() == b"yes");
        if lingering || checked(self.manager, "loginctl", &[OsStr::new("enable-linger")]).is_ok() {
            return None;
        }
        let user = std::env::var("USER").unwrap_or(uid);
        Some(format!(
            "Home runs while you are logged in; to keep it running after you log out, an administrator can run: sudo loginctl enable-linger {user}"
        ))
    }

    /// Waits until Home answers at its URL, or the manager reports a failed start.
    pub async fn wait_ready(&self) -> Result<String> {
        let deadline = Instant::now() + READY_TIMEOUT;
        loop {
            if let Some(url) = live_home_url(&self.spec.data_dir).await {
                return Ok(url);
            }
            match self.observe()? {
                (State::Failed(reason), _) => bail!("{reason}"),
                // Home exited cleanly: it refused to start (see its log).
                (State::Stopped, _) => bail!("Home stopped before it was ready"),
                (State::Running, restarts) if restarts > 0 => {
                    bail!("Home restarted {restarts} times while starting")
                }
                (State::Running, _) => {}
            }
            if Instant::now() >= deadline {
                bail!(
                    "Home did not become ready within {} seconds",
                    READY_TIMEOUT.as_secs()
                );
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
}

fn remove_file(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
        _ => Ok(()),
    }
}

/// The URL of a Home that answers now, from its private gateway coordinates.
pub async fn live_home_url(data_dir: &Path) -> Option<String> {
    let coords = crate::runtime_control::read_live_gateway_coords(data_dir).await?;
    (!coords.home_url.is_empty()).then_some(coords.home_url)
}

/// CLI updates replace the Runtime with the service stopped and start it again
/// afterwards, also when the update fails and restores the previous release.
pub struct UpdatePause {
    spec: Spec,
}

impl UpdatePause {
    pub fn begin(data_dir: &Path) -> Result<Option<Self>> {
        if service_mode() {
            return Ok(None);
        }
        let Some(platform) = Platform::current() else {
            return Ok(None);
        };
        let spec = Spec::current(platform, data_dir)?;
        let service = Service::new(spec.clone(), &SystemManager);
        if !service.running() {
            return Ok(None);
        }
        println!("  Stopping Home for the update...");
        service.stop().context("stop Home before the update")?;
        Ok(Some(Self { spec }))
    }
}

impl Drop for UpdatePause {
    fn drop(&mut self) {
        let service = Service::new(self.spec.clone(), &SystemManager);
        match service.start() {
            Ok(()) => println!("  Home is starting again in the background."),
            Err(error) => eprintln!(
                "  Home did not start again ({error:#}). Open it with: elastos home --browser"
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::Subcommand)]
pub enum ServiceCommand {
    /// Install Home as a background service of this user and start it
    Install {
        /// Open Home in the default browser once it is ready
        #[arg(long)]
        open: bool,
    },
    /// Stop the background Home until the next login or `elastos home --browser`
    Stop,
    /// Show whether the background Home is installed and running
    Status,
}

/// `elastos service ...`
pub async fn run_service_command(command: ServiceCommand) -> Result<()> {
    let platform = Platform::current().context("Home runs as a service only on Linux and macOS")?;
    let data_dir = crate::sources::default_data_dir();
    let spec = Spec::current(platform, &data_dir)?;
    let service = Service::new(spec, &SystemManager);
    match command {
        ServiceCommand::Install { open } => {
            let url = install_and_start(&service).await.map_err(|error| {
                // One line: the installer prints it and then opens Home in the terminal.
                anyhow::anyhow!("Home did not start as a background service: {error:#}")
            })?;
            if let Some(notice) = service.ensure_linger() {
                println!("{notice}");
            }
            println!("Home: {url}");
            if open {
                open_browser(&url);
            }
        }
        ServiceCommand::Stop => {
            if service.installed() {
                service.stop()?;
            }
            println!("Home is stopped. Open it again with: elastos home --browser");
        }
        ServiceCommand::Status => {
            if !service.installed() {
                println!("Home is not installed as a background service.");
            } else if let Some(url) = live_home_url(&data_dir).await {
                println!("Home runs in the background: {url}");
            } else {
                match service.state()? {
                    State::Failed(reason) => println!("Home's background service failed: {reason}."),
                    _ => println!("Home's background service is stopped. Start it with: elastos home --browser"),
                }
            }
        }
    }
    Ok(())
}

async fn install_and_start(service: &Service<'_>) -> Result<String> {
    require_installed_runtime(&service.spec)?;
    if !service.running() {
        if let Some(owner) = crate::host_lock::active_host_process(&service.spec.data_dir)? {
            bail!(
                "another Home already runs for this data folder (pid {}); close it first",
                owner.pid
            );
        }
    }
    service.install()?;
    service.wait_ready().await.inspect_err(|_| {
        let _ = service.remove();
    })
}

/// Opens a URL in the default browser, if this session has one.
pub fn open_browser(url: &str) {
    #[cfg(target_os = "linux")]
    {
        // Try WSL browser opener first, then xdg-open
        for cmd in &["wslview", "xdg-open"] {
            if Command::new(cmd)
                .arg(url)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .is_ok()
            {
                return;
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("open")
            .arg(url)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}

/// The service runs the installed Runtime at its canonical path only.
fn require_installed_runtime(spec: &Spec) -> Result<()> {
    let binary = spec.binary();
    let current = fs::canonicalize(std::env::current_exe()?)?;
    let canonical = fs::canonicalize(&binary).ok();
    anyhow::ensure!(
        canonical.as_deref() == Some(current.as_path()),
        "the background service runs the Runtime installed at {}; run the installer to put it there",
        binary.display()
    );
    let sources = crate::sources::load_trusted_sources(&spec.data_dir)?;
    if let Some(source) = sources.default_source() {
        anyhow::ensure!(
            source.install_path.is_empty() || Path::new(&source.install_path) == binary,
            "this Runtime is recorded at {}; run the installer to move it to {}",
            source.install_path,
            binary.display()
        );
    }
    Ok(())
}

/// The URL to open for `elastos home --browser` when Home already runs or its
/// service can start it. `None` means: start Home in this terminal as before.
pub async fn running_home_url(data_dir: &Path) -> Result<Option<String>> {
    if let Some(url) = live_home_url(data_dir).await {
        return Ok(Some(url));
    }
    if crate::update_controller::controller_active(data_dir) {
        // A controller owns Home and is starting it.
        let deadline = Instant::now() + READY_TIMEOUT;
        while Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if let Some(url) = live_home_url(data_dir).await {
                return Ok(Some(url));
            }
            if !crate::update_controller::controller_active(data_dir) {
                return Ok(None);
            }
        }
        bail!(
            "Home is starting but did not become ready within {} seconds; see {}",
            READY_TIMEOUT.as_secs(),
            data_dir.join("update-controller/runtime.log").display()
        );
    }
    let Some(platform) = Platform::current() else {
        return Ok(None);
    };
    let service = Service::new(Spec::current(platform, data_dir)?, &SystemManager);
    if !service.installed() {
        return Ok(None);
    }
    let started = async {
        service.start()?;
        service.wait_ready().await
    }
    .await;
    match started {
        Ok(url) => Ok(Some(url)),
        Err(error) => {
            eprintln!("Home could not start in the background ({error:#}); starting it in this terminal instead.");
            let _ = service.stop();
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests;
