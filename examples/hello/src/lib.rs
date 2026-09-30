//! Minimal portable ElastOS Component capsule.
//!
//! Runtime calls [`exports::elastos::bus::lifecycle::Guest::run`]. The capsule
//! reads Runtime facts through the ElastOS Bus and exits successfully when the
//! Component ABI is the expected version.

#[cfg(target_arch = "wasm32")]
wit_bindgen::generate!({
    path: "../../elastos/wit",
    world: "product-capsule-v1",
});

#[cfg(any(test, target_arch = "wasm32"))]
const EXPECTED_RUNTIME_ABI: &str = "elastos.component/v1";

#[cfg(target_arch = "wasm32")]
struct Hello;

#[cfg(target_arch = "wasm32")]
impl exports::elastos::bus::lifecycle::Guest for Hello {
    fn run() -> Result<(), elastos::bus::types::BusError> {
        let runtime = elastos::bus::runtime::info();
        validate_runtime_abi(&runtime.abi).map_err(elastos::bus::types::BusError::Invalid)?;

        // Reading identity demonstrates a Runtime-owned fact. This does not
        // request a capability or access a provider.
        let _identity = elastos::bus::identity::context();
        Ok(())
    }
}

#[cfg(any(test, target_arch = "wasm32"))]
fn validate_runtime_abi(runtime_abi: &str) -> Result<(), String> {
    if runtime_abi == EXPECTED_RUNTIME_ABI {
        return Ok(());
    }

    Err(format!(
        "hello requires {EXPECTED_RUNTIME_ABI}, received {runtime_abi}"
    ))
}

#[cfg(target_arch = "wasm32")]
export!(Hello);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_accepts_the_elastos_component_v1_runtime() {
        assert_eq!(validate_runtime_abi("elastos.component/v1"), Ok(()));
    }

    #[test]
    fn component_rejects_a_different_runtime_abi() {
        assert_eq!(
            validate_runtime_abi("wasi-preview1"),
            Err("hello requires elastos.component/v1, received wasi-preview1".to_string())
        );
    }
}
