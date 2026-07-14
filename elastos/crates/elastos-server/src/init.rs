//! Capsule scaffolding commands (`elastos init`).

use std::path::{Path, PathBuf};

const BUS_WIT: &str = include_str!("../../../wit/elastos-bus-v1.wit");

fn validate_capsule_name(name: &str) -> anyhow::Result<()> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if !valid {
        anyhow::bail!(
            "Capsule name must be 1-64 lowercase letters, numbers, or hyphens, without a leading or trailing hyphen"
        );
    }
    Ok(())
}

fn component_manifest(name: &str) -> serde_json::Value {
    serde_json::json!({
        "schema": elastos_common::SCHEMA_V1,
        "name": name,
        "version": "0.1.0",
        "description": format!("Run the {} app", name),
        "author": "local-development",
        "role": "app",
        "type": "wasm",
        "runtime_abi": "elastos.component/v1",
        "bus_contract": elastos_common::ELASTOS_BUS_V1_CONTRACT,
        "wit_world_sha256": elastos_common::elastos_bus_v1_wit_sha256(),
        "execution": "component",
        "projections": ["facts"],
        "entrypoint": format!("{}.component.wasm", name),
        "resources": {
            "memory_mb": 16,
            "gpu": false
        }
    })
}

fn content_manifest(name: &str) -> serde_json::Value {
    serde_json::json!({
        "schema": elastos_common::SCHEMA_V1,
        "name": name,
        "version": "0.1.0",
        "description": format!("{} documents", name),
        "author": "local-development",
        "role": "content",
        "type": "data",
        "entrypoint": "README.md",
        "viewer": "documents",
        "interfaces": [{
            "id": format!("{}.content", name),
            "version": "1.0.0",
            "description": format!("{} documents", name),
            "methods": [{
                "id": "document.open",
                "description": "Open these documents",
                "risk": "launch",
                "approval": "runtime_policy",
                "audit": "event",
                "resource": "elastos://documents/document",
                "operation": "open",
                "output_schema": {
                    "schema": "elastos.documents.content-opened/v1",
                    "content_type": "text/markdown",
                    "viewer": "documents"
                }
            }]
        }]
    })
}

/// Scaffold a WASM Component that uses the ElastOS Bus.
pub fn init_capsule(name: &str) -> anyhow::Result<()> {
    validate_capsule_name(name)?;
    let dir = PathBuf::from(name);
    if dir.exists() {
        anyhow::bail!("Directory '{}' already exists", name);
    }

    write_component_capsule(&dir, name)?;

    let core_artifact = name.replace('-', "_");
    println!("Created capsule '{}'", name);
    println!();
    println!("  cd {}", name);
    println!("  cargo build --release --target wasm32-unknown-unknown");
    println!(
        "  cargo run --quiet --manifest-path ../elastos/tools/componentize/Cargo.toml -- target/wasm32-unknown-unknown/release/{}.wasm {}.component.wasm",
        core_artifact, name
    );
    println!("  elastos run .");
    println!();

    Ok(())
}

fn write_component_capsule(dir: &Path, name: &str) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir.join("src"))?;
    std::fs::create_dir_all(dir.join("wit"))?;

    let capsule_json = component_manifest(name);
    std::fs::write(
        dir.join("capsule.json"),
        serde_json::to_string_pretty(&capsule_json)? + "\n",
    )?;
    std::fs::write(dir.join("wit/elastos-bus-v1.wit"), BUS_WIT)?;

    let cargo_toml = format!(
        r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2021"

[workspace]

[lib]
crate-type = ["cdylib"]

[dependencies]
wit-bindgen = "0.57.1"

[profile.release]
opt-level = "s"
lto = true
"#
    );
    std::fs::write(dir.join("Cargo.toml"), cargo_toml)?;

    let lib_rs = r#"wit_bindgen::generate!({
    path: "wit",
    world: "product-capsule-v1",
});

struct Capsule;

impl exports::elastos::bus::lifecycle::Guest for Capsule {
    fn run() -> Result<(), elastos::bus::types::BusError> {
        let _runtime = elastos::bus::runtime::info();
        let _identity = elastos::bus::identity::context();
        Ok(())
    }
}

export!(Capsule);
"#;
    std::fs::write(dir.join("src/lib.rs"), lib_rs)?;

    Ok(())
}

/// Scaffold a content capsule containing Markdown documents.
pub fn init_content_capsule(name: &str) -> anyhow::Result<()> {
    validate_capsule_name(name)?;
    let dir = PathBuf::from(name);
    if dir.exists() {
        anyhow::bail!("Directory '{}' already exists", name);
    }

    std::fs::create_dir_all(&dir)?;
    let capsule_json = content_manifest(name);
    std::fs::write(
        dir.join("capsule.json"),
        serde_json::to_string_pretty(&capsule_json)? + "\n",
    )?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use elastos_common::CapsuleManifest;

    #[test]
    fn generated_component_manifest_is_current_and_has_no_ambient_authority() {
        let value = component_manifest("sample-app");
        let manifest: CapsuleManifest = serde_json::from_value(value.clone()).unwrap();
        manifest.validate().unwrap();
        assert_eq!(
            value["wit_world_sha256"],
            elastos_common::elastos_bus_v1_wit_sha256()
        );
        assert!(value.get("permissions").is_none());
        assert!(value.get("capabilities").is_none());
        assert!(value.get("authority").is_none());
        assert!(value.get("interfaces").is_none());
    }

    #[test]
    fn generated_component_project_is_self_contained() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("sample-app");
        write_component_capsule(&dir, "sample-app").unwrap();

        assert!(dir.join("src/lib.rs").is_file());
        assert!(dir.join("wit/elastos-bus-v1.wit").is_file());
        assert!(!dir.join(".cargo/config.toml").exists());
        assert_eq!(
            std::fs::read_to_string(dir.join("wit/elastos-bus-v1.wit")).unwrap(),
            BUS_WIT
        );
        let manifest: CapsuleManifest =
            serde_json::from_str(&std::fs::read_to_string(dir.join("capsule.json")).unwrap())
                .unwrap();
        manifest.validate().unwrap();
    }

    #[test]
    fn generated_content_manifest_points_to_the_created_document() {
        let value = content_manifest("sample-docs");
        let manifest: CapsuleManifest = serde_json::from_value(value.clone()).unwrap();
        manifest.validate().unwrap();
        assert_eq!(value["entrypoint"], "README.md");
        assert_eq!(value["viewer"], "documents");
    }

    #[test]
    fn capsule_names_cannot_escape_the_requested_directory() {
        for name in ["../escape", "Uppercase", "-leading", "trailing-", ""] {
            assert!(validate_capsule_name(name).is_err(), "accepted {name:?}");
        }
        validate_capsule_name("useful-app-2").unwrap();
    }
}
