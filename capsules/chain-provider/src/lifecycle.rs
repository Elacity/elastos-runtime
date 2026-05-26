use super::*;
use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub(super) fn validate_node_supervisor_config(config: &NodeSupervisorConfig) -> Result<(), String> {
    for (network_id, network) in &config.networks {
        validate_network_id(network_id)?;
        validate_node_supervisor_command(&network.start, "start")?;
        validate_node_supervisor_command(&network.stop, "stop")?;
        validate_node_supervisor_command(&network.restart, "restart")?;
        if let Some(timeout_ms) = network.timeout_ms {
            if !(100..=120_000).contains(&timeout_ms) {
                return Err("node supervisor timeout_ms must be 100-120000".to_string());
            }
        }
    }
    Ok(())
}

pub(super) fn validate_node_supervisor_command(
    command: &NodeSupervisorCommand,
    field: &str,
) -> Result<(), String> {
    let program = command.program.trim();
    if program.is_empty()
        || !program.starts_with('/')
        || program.len() > 512
        || program
            .chars()
            .any(|ch| ch.is_ascii_control() || ch.is_ascii_whitespace())
    {
        return Err(format!(
            "{field} supervisor command must use an absolute program path"
        ));
    }
    for arg in &command.args {
        if arg.len() > 512 || arg.chars().any(|ch| ch == '\0' || ch.is_ascii_control()) {
            return Err(format!(
                "{field} supervisor command contains an invalid argument"
            ));
        }
    }
    Ok(())
}

pub(super) fn run_node_supervisor_action(
    supervisor: &NodeSupervisorNetworkConfig,
    action: NodeLifecycleAction,
) -> Result<(), Response> {
    let command = match action {
        NodeLifecycleAction::Status => return Ok(()),
        NodeLifecycleAction::Start => &supervisor.start,
        NodeLifecycleAction::Stop => &supervisor.stop,
        NodeLifecycleAction::Restart => &supervisor.restart,
    };
    run_node_supervisor_command(
        command,
        Duration::from_millis(supervisor.timeout_ms.unwrap_or(15_000)),
    )
}

pub(super) fn run_node_supervisor_command(
    command: &NodeSupervisorCommand,
    timeout: Duration,
) -> Result<(), Response> {
    let mut child = Command::new(&command.program)
        .args(&command.args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| Response::error("node_supervisor_unavailable", &err.to_string()))?;
    let started_at = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                return Err(Response::error(
                    "node_supervisor_failed",
                    &format!(
                        "node supervisor exited with status {}",
                        status.code().unwrap_or(-1)
                    ),
                ));
            }
            Ok(None) if started_at.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Response::error(
                    "node_supervisor_timeout",
                    "node supervisor command timed out",
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Response::error(
                    "node_supervisor_unavailable",
                    &err.to_string(),
                ));
            }
        }
    }
}

pub(super) fn data_dir() -> PathBuf {
    if let Ok(dir) = env::var("ELASTOS_DATA_DIR") {
        PathBuf::from(dir)
    } else if let Ok(dir) = env::var("XDG_DATA_HOME") {
        PathBuf::from(dir).join("elastos")
    } else if let Some(home) = env::var_os("HOME") {
        PathBuf::from(home).join(".local/share/elastos")
    } else {
        PathBuf::from("/tmp/elastos")
    }
}

pub(super) fn node_lifecycle_state_path(data_dir: &Path) -> PathBuf {
    data_dir
        .join("chain-provider")
        .join("node-lifecycle-state.json")
}

pub(super) fn read_node_lifecycle_state_file(
    path: &Path,
) -> Result<NodeLifecycleStateFile, String> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Ok(NodeLifecycleStateFile::default())
        }
        Err(err) => return Err(format!("read node lifecycle state: {err}")),
    };
    let state: NodeLifecycleStateFile = serde_json::from_str(&content)
        .map_err(|err| format!("parse node lifecycle state: {err}"))?;
    if state.schema != NODE_LIFECYCLE_STATE_SCHEMA {
        return Err("unsupported node lifecycle state schema".to_string());
    }
    for network_id in state.networks.keys() {
        validate_network_id(network_id)
            .map_err(|err| format!("invalid persisted network id {network_id}: {err}"))?;
    }
    Ok(state)
}

pub(super) fn write_node_lifecycle_state_file(
    path: &Path,
    state: &NodeLifecycleStateFile,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("create state directory: {err}"))?;
    }
    let json = serde_json::to_vec_pretty(state)
        .map_err(|err| format!("serialize node lifecycle state: {err}"))?;
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, json).map_err(|err| format!("write node lifecycle state: {err}"))?;
    fs::rename(&tmp, path).map_err(|err| format!("commit node lifecycle state: {err}"))?;
    Ok(())
}
