pub fn run_init(name: String, capsule_type: String) -> anyhow::Result<()> {
    match capsule_type.as_str() {
        "wasm" => elastos_server::init::init_capsule(&name)?,
        "content" => elastos_server::init::init_content_capsule(&name)?,
        _ => anyhow::bail!(
            "Unknown capsule type '{}'. Supported: wasm, content",
            capsule_type
        ),
    }

    Ok(())
}
