use std::path::Path;

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
            let mut table: toml::Table = contents.parse().unwrap_or_default();
            let toml_val = if let Ok(b) = value.parse::<bool>() {
                toml::Value::Boolean(b)
            } else if let Ok(n) = value.parse::<i64>() {
                toml::Value::Integer(n)
            } else {
                toml::Value::String(value)
            };
            table.insert(key.clone(), toml_val);
            std::fs::write(&config_path, toml::to_string_pretty(&table)?)?;
            println!("Set {} in {}", key, config_path.display());
        }
    }
    Ok(())
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
