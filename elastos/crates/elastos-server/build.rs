use std::env;

fn main() {
    println!("cargo:rerun-if-env-changed=ELASTOS_RELEASE_VERSION");

    let version = env::var("ELASTOS_RELEASE_VERSION").unwrap_or_else(|_| {
        let pkg = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.1.0".to_string());
        format!("{pkg}-dev")
    });

    println!("cargo:rustc-env=ELASTOS_VERSION={version}");
}
