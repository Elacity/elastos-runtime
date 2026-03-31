//! Capsule scaffolding commands (`elastos init`).

use std::path::PathBuf;

enum GuestDependencySource {
    SourceTree(PathBuf),
    InstalledSdk(PathBuf),
    RegistryVersion(String),
}

fn resolve_guest_dependency_source() -> GuestDependencySource {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let source_tree = manifest_dir.join("../elastos-guest").canonicalize().ok();
    if let Some(path) = source_tree {
        return GuestDependencySource::SourceTree(path);
    }

    let sdk_path = std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".local/share/elastos/sdk/elastos-guest"))
        .filter(|p| p.join("Cargo.toml").is_file());
    if let Some(path) = sdk_path {
        return GuestDependencySource::InstalledSdk(path);
    }

    GuestDependencySource::RegistryVersion("0.1".to_string())
}

/// Scaffold a new WASM capsule project.
pub fn init_capsule(name: &str) -> anyhow::Result<()> {
    let dir = PathBuf::from(name);
    if dir.exists() {
        anyhow::bail!("Directory '{}' already exists", name);
    }

    // Create directory structure
    std::fs::create_dir_all(dir.join("src"))?;
    std::fs::create_dir_all(dir.join(".cargo"))?;

    // capsule.json
    let capsule_json = serde_json::json!({
        "schema": elastos_common::SCHEMA_V1,
        "version": "0.1.0",
        "name": name,
        "description": format!("A {} capsule", name),
        "author": "",
        "type": "wasm",
        "entrypoint": format!("{}.wasm", name),
        "requires": [],
        "capabilities": [],
        "resources": {
            "memory_mb": 16,
            "cpu_shares": 50
        },
        "permissions": {
            "storage": [],
            "messaging": []
        }
    });
    std::fs::write(
        dir.join("capsule.json"),
        serde_json::to_string_pretty(&capsule_json)? + "\n",
    )?;

    let guest_dep_source = resolve_guest_dependency_source();
    let guest_dep = match &guest_dep_source {
        GuestDependencySource::SourceTree(path) | GuestDependencySource::InstalledSdk(path) => {
            format!("elastos-guest = {{ path = \"{}\" }}", path.display())
        }
        GuestDependencySource::RegistryVersion(version) => {
            format!("elastos-guest = \"{}\"", version)
        }
    };

    let cargo_toml = format!(
        r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2021"

[workspace]

[[bin]]
name = "{name}"
path = "src/main.rs"

[dependencies]
{guest_dep}

[profile.release]
opt-level = "s"
lto = true
"#
    );
    std::fs::write(dir.join("Cargo.toml"), cargo_toml)?;

    // .cargo/config.toml
    let cargo_config = r#"[build]
target = "wasm32-wasip1"
"#;
    std::fs::write(dir.join(".cargo").join("config.toml"), cargo_config)?;

    // src/main.rs
    let main_rs = r#"use elastos_guest::prelude::*;

fn main() {
    let info = CapsuleInfo::from_env();

    if info.is_elastos_runtime() {
        log!("Running inside ElastOS runtime");
        println!("Capsule: {} ({})", info.name(), info.id());
    }

    println!("Hello from ElastOS!");
}
"#;
    std::fs::write(dir.join("src").join("main.rs"), main_rs)?;

    println!("Created capsule '{}'", name);
    println!();
    println!("  cd {}", name);
    println!("  cargo build --release");
    println!("  cp target/wasm32-wasip1/release/{}.wasm .", name);
    println!("  elastos run .");
    println!();

    match guest_dep_source {
        GuestDependencySource::SourceTree(_) => {}
        GuestDependencySource::InstalledSdk(_) => {
            println!("  Note: using installed elastos-guest SDK from ~/.local/share/elastos/sdk.");
            println!();
        }
        GuestDependencySource::RegistryVersion(version) => {
            println!(
                "  Note: using crates.io elastos-guest = \"{}\" because no source-tree or installed SDK path was found.",
                version
            );
            println!("  Install ElastOS or create capsules inside the elastos/ workspace for a local SDK path.");
            println!();
        }
    }

    Ok(())
}

/// Scaffold a new content capsule (markdown documents with md-viewer).
pub fn init_content_capsule(name: &str) -> anyhow::Result<()> {
    let dir = PathBuf::from(name);
    if dir.exists() {
        anyhow::bail!("Directory '{}' already exists", name);
    }

    std::fs::create_dir_all(&dir)?;

    // capsule.json
    let capsule_json = serde_json::json!({
        "schema": elastos_common::SCHEMA_V1,
        "version": "0.1.0",
        "name": name,
        "description": format!("{} — shared documents", name),
        "type": "data",
        "entrypoint": "index.html",
        "viewer": "md-viewer",
        "requires": [],
        "capabilities": [],
        "resources": { "memory_mb": 16, "cpu_shares": 50 },
        "permissions": { "storage": [], "messaging": [] }
    });
    std::fs::write(
        dir.join("capsule.json"),
        serde_json::to_string_pretty(&capsule_json)? + "\n",
    )?;

    // README.md
    let readme = format!(
        "# {}\n\nWelcome to your ElastOS content capsule.\n\n\
         Add `.md` files to this directory, then share:\n\n\
         ```bash\n\
         elastos share {}\n\
         ```\n",
        name, name
    );
    std::fs::write(dir.join("README.md"), readme)?;

    println!("Created content capsule '{}'", name);
    println!();
    println!("  cd {}", name);
    println!("  # Add .md files, then:");
    println!("  elastos share .");
    println!();

    Ok(())
}
