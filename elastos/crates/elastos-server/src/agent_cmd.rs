use std::process::{Command, Stdio};

use anyhow::Context;

use elastos_server::sources::default_data_dir;

use crate::shell_cmd;

pub async fn run_agent(
    nick: Option<String>,
    channel: String,
    backend: String,
    respond_all: bool,
    connect: Option<String>,
) -> anyhow::Result<()> {
    let nick = Some(resolve_agent_persona_name(nick, &backend)?);

    if backend == "codex" {
        return run_host_codex_agent(nick, channel, backend, respond_all, connect).await;
    }

    let cmd = serde_json::json!({
        "command": "agent",
        "nick": nick,
        "channel": channel,
        "backend": backend,
        "connect": connect,
        "respond_all": respond_all,
    });
    shell_cmd::forward_to_shell(cmd).await
}

async fn run_host_codex_agent(
    nick: Option<String>,
    channel: String,
    backend: String,
    respond_all: bool,
    connect: Option<String>,
) -> anyhow::Result<()> {
    let data_dir = default_data_dir();
    let (coords, client_token) =
        shell_cmd::attach_client_token_to_operator_runtime(&data_dir).await?;

    let agent_bin = crate::resolve_verified_provider_binary(
        "agent",
        "agent binary not installed.\n\nRun first:\n\n  elastos setup --with agent",
    )?;

    let mut child = Command::new(&agent_bin);
    if let Some(nick) = nick {
        child.arg("--nick").arg(nick);
    }
    child.arg("--channel").arg(channel);
    child.arg("--backend").arg(backend);
    if respond_all {
        child.arg("--respond-all");
    }
    if let Some(connect) = connect {
        child.arg("--connect").arg(connect);
    }
    child
        .env("ELASTOS_API", coords.api_url)
        .env("ELASTOS_TOKEN", client_token)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    let status = child
        .status()
        .with_context(|| format!("failed to start host agent binary: {}", agent_bin.display()))?;

    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("host agent exited with status {}", status);
    }
}

fn resolve_agent_persona_name(nick: Option<String>, backend: &str) -> anyhow::Result<String> {
    if let Some(nick) = nick {
        let trimmed = nick.trim();
        if trimmed.is_empty() {
            anyhow::bail!("agent persona name must not be empty");
        }
        return Ok(trimmed.to_string());
    }

    let backend = backend.trim().to_ascii_lowercase();
    if !backend.is_empty()
        && backend.len() <= 64
        && backend
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Ok(backend);
    }

    Ok("agent".to_string())
}

#[cfg(test)]
mod tests {
    use super::resolve_agent_persona_name;

    #[test]
    fn explicit_agent_persona_name_wins() {
        assert_eq!(
            resolve_agent_persona_name(Some("codex-ops".to_string()), "codex").unwrap(),
            "codex-ops"
        );
    }

    #[test]
    fn codex_backend_defaults_to_codex_persona() {
        assert_eq!(resolve_agent_persona_name(None, "codex").unwrap(), "codex");
    }

    #[test]
    fn invalid_backend_falls_back_to_agent_persona() {
        assert_eq!(
            resolve_agent_persona_name(None, "not valid").unwrap(),
            "agent"
        );
    }

    #[test]
    fn empty_agent_persona_name_is_rejected() {
        assert!(resolve_agent_persona_name(Some("   ".to_string()), "codex").is_err());
    }
}
