use std::io::{self, IsTerminal, Write};
use std::path::Path;

use anyhow::Context;

use elastos_server::sources::default_data_dir;

#[derive(Debug, Clone, Default)]
pub(crate) struct IdentityProfile {
    pub(crate) did: Option<String>,
    pub(crate) nickname: Option<String>,
}

pub async fn run_identity(cmd: crate::IdentityCommand) -> anyhow::Result<()> {
    match cmd {
        crate::IdentityCommand::Show => {
            let profile = load_identity_profile(&default_data_dir()).await?;
            print_identity_profile(&profile)?;
        }
        crate::IdentityCommand::Nickname(cmd) => match cmd {
            crate::IdentityNicknameCommand::Get => {
                let profile = load_identity_profile(&default_data_dir()).await?;
                if let Some(nick) = profile.nickname {
                    println!("{}", nick);
                }
            }
            crate::IdentityNicknameCommand::Set { value } => {
                let nick = set_local_nickname(&default_data_dir(), value).await?;
                println!("Nickname set to '{}'.", nick);
            }
        },
    }
    Ok(())
}

pub(crate) async fn load_identity_profile(data_dir: &Path) -> anyhow::Result<IdentityProfile> {
    let did = elastos_identity::load_or_create_did(data_dir)
        .ok()
        .map(|(_, did)| did)
        .filter(|did| !did.trim().is_empty());
    let nickname = elastos_identity::load_nickname(data_dir).ok().flatten();
    Ok(IdentityProfile { did, nickname })
}

pub(crate) async fn set_local_nickname(
    data_dir: &Path,
    value: Option<String>,
) -> anyhow::Result<String> {
    let current = load_identity_profile(data_dir).await.unwrap_or_default();
    let value = resolve_nickname_input(value, current.nickname.as_deref())?;
    validate_nickname(&value)?;
    elastos_identity::save_nickname(data_dir, &value)?;
    Ok(value)
}

fn print_identity_profile(profile: &IdentityProfile) -> anyhow::Result<()> {
    let mut out = io::stdout().lock();
    writeln!(
        out,
        "Profile:   {}",
        if profile.did.is_some() {
            "initialized"
        } else {
            "not initialized yet"
        }
    )?;
    writeln!(
        out,
        "DID:       {}",
        profile.did.as_deref().unwrap_or("(not initialized yet)")
    )?;
    writeln!(
        out,
        "Nickname:  {}",
        profile.nickname.as_deref().unwrap_or("(not set)")
    )?;
    Ok(())
}

fn resolve_nickname_input(value: Option<String>, current: Option<&str>) -> anyhow::Result<String> {
    match value {
        Some(value) => Ok(value.trim().to_string()),
        None => prompt_for_nickname(current),
    }
}

fn prompt_for_nickname(current: Option<&str>) -> anyhow::Result<String> {
    if !(io::stdin().is_terminal() && io::stdout().is_terminal()) {
        anyhow::bail!("nickname value missing; pass a value or run in an interactive terminal");
    }

    let prompt = current
        .filter(|nick| !nick.is_empty())
        .map(|nick| format!("Nickname [{}]: ", nick))
        .unwrap_or_else(|| "Nickname: ".to_string());
    print!("{}", prompt);
    io::stdout().flush()?;

    let mut input = String::new();
    let read = io::stdin()
        .read_line(&mut input)
        .context("failed to read nickname")?;
    if read == 0 {
        anyhow::bail!("nickname prompt ended before a value was provided");
    }

    let trimmed = input.trim().to_string();
    if trimmed.is_empty() {
        if let Some(current) = current.filter(|nick| !nick.is_empty()) {
            return Ok(current.to_string());
        }
    }
    Ok(trimmed)
}

fn validate_nickname(nickname: &str) -> anyhow::Result<()> {
    let trimmed = nickname.trim();
    if trimmed.is_empty() {
        anyhow::bail!("nickname must not be empty");
    }
    if trimmed.chars().count() > 32 {
        anyhow::bail!("nickname must be 32 characters or fewer");
    }
    if trimmed.chars().any(|ch| ch.is_control()) {
        anyhow::bail!("nickname must not contain control characters");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{prompt_for_nickname, validate_nickname};
    use std::io::IsTerminal;

    #[test]
    fn nickname_validation_rejects_empty() {
        assert!(validate_nickname("   ").is_err());
    }

    #[test]
    fn nickname_validation_rejects_control_chars() {
        assert!(validate_nickname("bad\nnick").is_err());
    }

    #[test]
    fn nickname_validation_accepts_simple_value() {
        assert!(validate_nickname("anders").is_ok());
    }

    #[test]
    fn prompt_without_tty_and_without_value_would_fail() {
        // The helper must not silently invent a nickname when there is no tty.
        if !(std::io::stdin().is_terminal() && std::io::stdout().is_terminal()) {
            assert!(prompt_for_nickname(None).is_err());
        }
    }
}
