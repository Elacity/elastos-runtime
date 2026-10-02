use std::path::Path;

use anyhow::Context as _;

use elastos_server::sources::default_data_dir;

pub fn run_config(cmd: crate::ConfigCommand) -> anyhow::Result<()> {
    let data_dir = default_data_dir();
    let config_path = data_dir.join("config.toml");
    match cmd {
        crate::ConfigCommand::Show => {
            if config_path.exists() {
                let contents = std::fs::read_to_string(&config_path)?;
                print!("{}", render_config_show(&config_path, &contents));
            } else {
                println!("No config file found. Run `elastos serve` to create one at:");
                println!("  {}", config_path.display());
            }
        }
        crate::ConfigCommand::Set { key, value } => {
            let contents = if config_path.exists() {
                std::fs::read_to_string(&config_path)?
            } else {
                let _ = std::fs::create_dir_all(&data_dir);
                String::new()
            };
            let updated = updated_config(&contents, &key, &value)?;
            std::fs::write(&config_path, updated)?;
            println!("Set {} in {}", key, config_path.display());
        }
    }
    Ok(())
}

fn updated_config(contents: &str, key: &str, value: &str) -> anyhow::Result<String> {
    if key == "developer_mode" {
        value
            .parse::<bool>()
            .context("developer_mode requires true or false")?;
    }
    if key == "gateway_public_publisher_bootstrap" {
        value
            .parse::<bool>()
            .context("gateway_public_publisher_bootstrap requires true or false")?;
    }
    if key == "carrier_bind_addr" {
        value
            .parse::<std::net::SocketAddr>()
            .context("Invalid carrier_bind_addr")?;
    }
    let mut table: toml::Table = contents
        .parse()
        .context("Invalid config.toml; existing settings preserved")?;
    let toml_val = if key == "gateway_allowed_hosts" {
        let hosts: Vec<String> = serde_json::from_str(value)
            .context("gateway_allowed_hosts requires a JSON array of authorities")?;
        if hosts.len() > 32 {
            anyhow::bail!("gateway_allowed_hosts exceeds 32 names");
        }
        for host in &hosts {
            let authority = host
                .parse::<axum::http::uri::Authority>()
                .context("invalid gateway host")?;
            if host.trim() != host
                || host.contains(['/', '@', '?', '#'])
                || authority.host().ends_with('.')
                || authority.host().contains('%')
                || authority.as_str() != authority.host() && authority.port_u16().is_none()
            {
                anyhow::bail!("invalid gateway host");
            }
        }
        toml::Value::Array(hosts.into_iter().map(toml::Value::String).collect())
    } else if let Ok(b) = value.parse::<bool>() {
        toml::Value::Boolean(b)
    } else if let Ok(n) = value.parse::<i64>() {
        toml::Value::Integer(n)
    } else {
        toml::Value::String(value.to_string())
    };
    table.insert(key.to_string(), toml_val);
    Ok(toml::to_string_pretty(&table)?)
}

fn render_config_show(path: &Path, contents: &str) -> String {
    let mut out = format!("# {}\n", path.display());
    if contents.trim().is_empty() {
        out.push_str("# (empty config file)\n");
    } else {
        out.push_str(contents);
        if !contents.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn developer_mode_update_requires_a_boolean_and_preserves_settings() {
        for enabled in [true, false] {
            let updated = super::updated_config(
                "dev_mode = true\ncarrier_bind_addr = '127.0.0.1:61967'\n",
                "developer_mode",
                &enabled.to_string(),
            )
            .unwrap();
            let table: toml::Table = updated.parse().unwrap();
            assert_eq!(table["developer_mode"].as_bool(), Some(enabled));
            assert_eq!(table["dev_mode"].as_bool(), Some(true));
            assert_eq!(table["carrier_bind_addr"].as_str(), Some("127.0.0.1:61967"));
        }
        for invalid in ["1", "yes", "True", ""] {
            assert!(super::updated_config("", "developer_mode", invalid).is_err());
        }
    }

    #[test]
    fn gateway_hosts_are_explicit_authorities_and_preserve_other_settings() {
        let updated = super::updated_config(
            "dev_mode = true\n",
            "gateway_allowed_hosts",
            r#"["home.example.test", "127.0.0.1:61180", "[::1]:61180"]"#,
        )
        .unwrap();
        let table: toml::Table = updated.parse().unwrap();
        assert_eq!(table["dev_mode"].as_bool(), Some(true));
        assert_eq!(table["gateway_allowed_hosts"].as_array().unwrap().len(), 3);
        for invalid in [
            r#"["https://home.example.test"]"#,
            r#"["home.example.test."]"#,
            r#"["user@home.example.test"]"#,
            r#"["home.example.test:99999"]"#,
            r#"[" home.example.test"]"#,
            r#"[1]"#,
        ] {
            assert!(
                super::updated_config("dev_mode = true", "gateway_allowed_hosts", invalid).is_err(),
                "{invalid}"
            );
        }
        let too_many = serde_json::to_string(&vec!["home.example.test"; 33]).unwrap();
        assert!(super::updated_config("", "gateway_allowed_hosts", &too_many).is_err());
    }

    #[test]
    fn public_publisher_bootstrap_requires_an_explicit_boolean() {
        for value in ["yes", "1", "TRUE", ""] {
            assert!(
                super::updated_config("", "gateway_public_publisher_bootstrap", value).is_err()
            );
        }
        let updated =
            super::updated_config("", "gateway_public_publisher_bootstrap", "true").unwrap();
        let table: toml::Table = updated.parse().unwrap();
        assert_eq!(
            table["gateway_public_publisher_bootstrap"].as_bool(),
            Some(true)
        );
    }

    #[test]
    fn carrier_binding_update_preserves_settings_and_rejects_invalid_input() {
        let updated =
            super::updated_config("dev_mode = true\n", "carrier_bind_addr", "127.0.0.1:61967")
                .unwrap();
        let table: toml::Table = updated.parse().unwrap();
        assert_eq!(table["dev_mode"].as_bool(), Some(true));
        assert_eq!(table["carrier_bind_addr"].as_str(), Some("127.0.0.1:61967"));
        assert!(super::updated_config("dev_mode = true", "carrier_bind_addr", "invalid").is_err());
        assert!(
            super::updated_config("broken = [", "carrier_bind_addr", "127.0.0.1:61967").is_err()
        );
    }

    use std::path::Path;

    use super::render_config_show;

    #[test]
    fn render_config_show_marks_empty_file() {
        let rendered = render_config_show(Path::new("/tmp/config.toml"), "");
        assert!(rendered.contains("# /tmp/config.toml"));
        assert!(rendered.contains("# (empty config file)"));
    }

    #[test]
    fn render_config_show_preserves_non_empty_contents() {
        let rendered = render_config_show(Path::new("/tmp/config.toml"), "dev_mode = true");
        assert!(rendered.contains("# /tmp/config.toml"));
        assert!(rendered.contains("dev_mode = true"));
        assert!(!rendered.contains("(empty config file)"));
        assert!(rendered.ends_with('\n'));
    }
}
