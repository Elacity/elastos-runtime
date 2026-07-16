wit_bindgen::generate!({
    path: "../../../../wit",
    world: "product-capsule-v1",
});

const RESOURCE: &str = "elastos://test/bus/probe";
const PRINCIPAL: &str = "person:local:bus-conformance";

struct BusConformance;

impl exports::elastos::bus::lifecycle::Guest for BusConformance {
    fn run() -> Result<(), elastos::bus::types::BusError> {
        let runtime = elastos::bus::runtime::info();
        if runtime.abi != "elastos.component/v1" {
            return Err(elastos::bus::types::BusError::Invalid(format!(
                "unexpected component ABI {}",
                runtime.abi
            )));
        }

        let identity = elastos::bus::identity::context();
        if identity.principal.as_deref() != Some(PRINCIPAL) {
            return Err(elastos::bus::types::BusError::Denied(
                "Runtime did not bind the conformance principal".to_string(),
            ));
        }

        let grant = elastos::bus::capabilities::request(
            &elastos::bus::types::CapabilityRequest {
                resource: RESOURCE.to_string(),
                actions: vec!["read".to_string()],
                reason: "verify the Component-to-Bus authority path".to_string(),
            },
        )?;

        let response =
            elastos::bus::providers::invoke(&elastos::bus::types::InvokeRequest {
                resource: RESOURCE.to_string(),
                operation: "read".to_string(),
                body: br#"{"probe":"bus-v1-conformance"}"#.to_vec(),
                grant: Some(grant.id),
            })?;

        if response.status != "ok" {
            return Err(elastos::bus::types::BusError::Invalid(format!(
                "provider returned status {}",
                response.status
            )));
        }
        if response.audit.is_none() {
            return Err(elastos::bus::types::BusError::Invalid(
                "Runtime omitted the invocation audit receipt".to_string(),
            ));
        }

        Ok(())
    }
}

export!(BusConformance);
