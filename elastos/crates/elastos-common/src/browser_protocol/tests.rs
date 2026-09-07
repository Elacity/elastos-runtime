use super::*;
use serde_json::json;

fn inventory() -> Value {
    json!({
        "provider": BROWSER_ENGINE_PROVIDER_ID,
        "protocol_version": BROWSER_ENGINE_PROTOCOL_VERSION,
        "status": "configured",
        "adapter_count": 2,
        "direct_network": false,
        "wallet_injection": false,
        "adapters": [
            {
                "id": "policy-engine", "engine": "cef", "default": true,
                "backing_substrate": "host_policy_webview",
                "supported_display_modes": ["native_surface"],
                "supported_guarantee_levels": ["policy_webview"],
                "network_mode": "runtime_net_only", "direct_network": false,
                "wallet_injection": false
            },
            {
                "id": "isolated-engine", "engine": "chromium_microvm", "default": false,
                "backing_substrate": "local_microvm",
                "supported_display_modes": ["webrtc_remote_display"],
                "supported_guarantee_levels": ["mechanism_microvm"],
                "network_mode": "runtime_net_only", "direct_network": false,
                "wallet_injection": false
            }
        ]
    })
}

#[test]
fn automatic_selection_obeys_requested_capabilities_instead_of_first_adapter() {
    let parsed = BrowserEngineInventory::from_status(&inventory()).unwrap();
    assert_eq!(
        parsed
            .select(
                None,
                BrowserDisplayMode::WebrtcRemoteDisplay,
                BrowserGuaranteeLevel::MechanismMicrovm
            )
            .unwrap()
            .id,
        "isolated-engine"
    );
    assert_eq!(
        parsed
            .select(
                None,
                BrowserDisplayMode::NativeSurface,
                BrowserGuaranteeLevel::PolicyWebview
            )
            .unwrap()
            .id,
        "policy-engine"
    );
}

#[test]
fn explicit_selection_preserves_operator_choice_and_isolation() {
    let parsed = BrowserEngineInventory::from_status(&inventory()).unwrap();
    assert_eq!(
        parsed.select(
            Some("policy-engine"),
            BrowserDisplayMode::WebrtcRemoteDisplay,
            BrowserGuaranteeLevel::MechanismMicrovm
        ),
        Err(BrowserCompatibilityError::IncompatibleEngineCapabilities)
    );
    assert_eq!(
        parsed.select(
            Some("absent"),
            BrowserDisplayMode::WebrtcRemoteDisplay,
            BrowserGuaranteeLevel::MechanismMicrovm
        ),
        Err(BrowserCompatibilityError::EngineNotFound)
    );
    assert_eq!(
        parsed.select(
            None,
            BrowserDisplayMode::WebrtcRemoteDisplay,
            BrowserGuaranteeLevel::OperatorRbi
        ),
        Err(BrowserCompatibilityError::NoCompatibleEngine)
    );
}

#[test]
fn placement_does_not_change_compatibility_or_permission_requirements() {
    for substrate in ["local_microvm", "remote_operator_vm", "remote_runtime"] {
        let mut data = inventory();
        data["adapters"][1]["backing_substrate"] = json!(substrate);
        let parsed = BrowserEngineInventory::from_status(&data).unwrap();
        assert_eq!(
            parsed
                .select(
                    None,
                    BrowserDisplayMode::WebrtcRemoteDisplay,
                    BrowserGuaranteeLevel::MechanismMicrovm
                )
                .unwrap()
                .id,
            "isolated-engine"
        );
        assert_eq!(
            parsed.select(
                None,
                BrowserDisplayMode::NativeSurface,
                BrowserGuaranteeLevel::MechanismMicrovm
            ),
            Err(BrowserCompatibilityError::NoCompatibleEngine)
        );
    }
}

#[test]
fn incompatible_versions_are_classified_before_version_specific_fields() {
    for version in [
        Value::Null,
        json!("1.0"),
        json!("2.1"),
        json!("3.0"),
        json!(2),
    ] {
        let mut data = inventory();
        data["protocol_version"] = version;
        data["adapters"] = json!("a future protocol might have a different shape");
        assert_eq!(
            BrowserEngineInventory::from_status(&data).unwrap_err(),
            BrowserCompatibilityError::IncompatibleEngineProtocol
        );
    }
}

#[test]
fn malformed_or_widened_inventory_is_rejected_before_selection() {
    for (pointer, value) in [
        ("/provider", json!("foreign-provider")),
        ("/direct_network", json!(true)),
        ("/wallet_injection", json!(true)),
        ("/adapter_count", json!(3)),
        ("/status", json!("ready")),
        ("/adapters/0/id", json!("../private")),
        ("/adapters/0/engine", json!("/private/engine")),
        ("/adapters/0/backing_substrate", json!("http://private/")),
        ("/adapters/0/direct_network", json!(true)),
        ("/adapters/0/wallet_injection", json!(true)),
        ("/adapters/0/network_mode", json!("direct")),
        ("/adapters/0/supported_display_modes", json!([])),
        (
            "/adapters/0/supported_display_modes",
            json!(["native_surface", "native_surface"]),
        ),
        (
            "/adapters/0/supported_guarantee_levels",
            json!(["diagnostic"]),
        ),
        ("/adapters/1/default", json!(true)),
        ("/adapters/1/id", json!("policy-engine")),
    ] {
        let mut data = inventory();
        *data.pointer_mut(pointer).unwrap() = value;
        assert!(
            BrowserEngineInventory::from_status(&data).is_err(),
            "{pointer}"
        );
    }
    let mut oversized = inventory();
    oversized["adapters"] = json!(vec![oversized["adapters"][0].clone(); 65]);
    oversized["adapter_count"] = json!(65);
    assert!(BrowserEngineInventory::from_status(&oversized).is_err());
}

#[test]
fn absent_capabilities_are_not_inferred_from_adapter_kind() {
    for field in [
        "supported_display_modes",
        "supported_guarantee_levels",
        "network_mode",
        "default",
    ] {
        let mut data = inventory();
        data["adapters"][0].as_object_mut().unwrap().remove(field);
        assert!(
            BrowserEngineInventory::from_status(&data).is_err(),
            "{field}"
        );
    }
}

#[test]
fn unavailable_provider_has_an_empty_inventory_and_typed_result() {
    let mut data = inventory();
    data["status"] = json!("unavailable");
    data["adapters"] = json!([]);
    data["adapter_count"] = json!(0);
    let parsed = BrowserEngineInventory::from_status(&data).unwrap();
    let error = parsed
        .select(
            None,
            BrowserDisplayMode::WebrtcRemoteDisplay,
            BrowserGuaranteeLevel::MechanismMicrovm,
        )
        .unwrap_err();
    assert_eq!(
        serde_json::to_value(error).unwrap(),
        json!({"code": "engine_unavailable"})
    );
}

#[test]
fn public_inventory_projection_drops_private_provider_fields() {
    let mut data = inventory();
    data["adapters"][0]["connect_ticket"] = json!("private-ticket");
    data["adapters"][0]["disk_path"] = json!("/private/profile");
    let parsed = BrowserEngineInventory::from_status(&data).unwrap();
    let serialized = serde_json::to_value(parsed).unwrap();
    assert!(serialized["adapters"][0].get("connect_ticket").is_none());
    assert!(serialized["adapters"][0].get("disk_path").is_none());
}

#[test]
fn operator_open_intent_cannot_supply_runtime_or_profile_authority() {
    let request = json!({
        "url": "https://example.com/", "display_mode": "webrtc_remote_display",
        "guarantee_level": "mechanism_microvm", "async_open": true
    });
    let parsed: BrowserOpenRequest = serde_json::from_value(request.clone()).unwrap();
    assert_eq!(parsed.display_mode, BrowserDisplayMode::WebrtcRemoteDisplay);
    for field in [
        "principal_id",
        "session_id",
        "profile",
        "disk_path",
        "connect_ticket",
        "provider_url",
    ] {
        let mut substituted = request.clone();
        substituted[field] = json!("caller-selected-authority");
        assert!(
            serde_json::from_value::<BrowserOpenRequest>(substituted).is_err(),
            "{field}"
        );
    }
}
