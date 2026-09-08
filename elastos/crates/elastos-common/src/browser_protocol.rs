//! Browser wire types shared by Runtime and its Engine provider.
//!
//! These types describe capabilities and page semantics. Host detection, service
//! grants, installation and transport remain with Runtime and host adapters.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

pub const BROWSER_ENGINE_PROVIDER_ID: &str = "browser-engine-adapter";
pub const BROWSER_ENGINE_PROTOCOL_VERSION: &str = "2.1";
pub const BROWSER_ENGINE_CLEANUP_BINDING_SCHEMA: &str = "elastos.browser.engine-cleanup-binding/v2";
pub const BROWSER_ENGINE_CLEANUP_RESULT_SCHEMA: &str = "elastos.browser.engine-cleanup-result/v2";
pub const MAX_BROWSER_ENGINE_ADAPTERS: usize = 64;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserDisplayMode {
    WebrtcRemoteDisplay,
    NativeSurface,
}

impl BrowserDisplayMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WebrtcRemoteDisplay => "webrtc_remote_display",
            Self::NativeSurface => "native_surface",
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserGuaranteeLevel {
    MechanismMicrovm,
    OperatorRbi,
    PolicyWebview,
    /// Recognized so Runtime can give an explicit rejection to old callers.
    Diagnostic,
}

impl BrowserGuaranteeLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MechanismMicrovm => "mechanism_microvm",
            Self::OperatorRbi => "operator_rbi",
            Self::PolicyWebview => "policy_webview",
            Self::Diagnostic => "diagnostic",
        }
    }

    pub fn supports_display(self, display: BrowserDisplayMode) -> bool {
        matches!(
            (self, display),
            (
                Self::MechanismMicrovm | Self::OperatorRbi,
                BrowserDisplayMode::WebrtcRemoteDisplay
            ) | (Self::PolicyWebview, BrowserDisplayMode::NativeSurface)
        )
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BrowserViewport {
    pub width: u32,
    pub height: u32,
}

/// Capsule/operator requests contain intent. Runtime obtains the principal,
/// session, provider route and profile authority from the verified launch grant.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserOpenRequest {
    pub url: String,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub remote_exit_id: Option<String>,
    #[serde(default)]
    pub adapter_id: Option<String>,
    #[serde(default)]
    pub browser_instance: Option<String>,
    #[serde(default)]
    pub viewport: Option<BrowserViewport>,
    pub display_mode: BrowserDisplayMode,
    pub guarantee_level: BrowserGuaranteeLevel,
    #[serde(default)]
    pub async_open: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserInputRequest {
    /// Engine-owned input-event schema, validated by the selected page adapter.
    pub event: Value,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserWebrtcSignalRequest {
    #[serde(rename = "type")]
    pub signal_type: String,
    #[serde(default)]
    pub channel: Option<String>,
    #[serde(default)]
    pub sdp: Option<String>,
    #[serde(default)]
    pub candidate: Option<Value>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserPageCloseRequest {
    pub schema: String,
    pub cleanup_id: String,
    #[serde(default)]
    pub browser_instance: Option<String>,
}

/// Private Runtime-to-Engine descriptor. Runtime projects a separate public
/// profile view; disk paths belong to the provider's host adaptation boundary.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserProfileDescriptor {
    pub schema: String,
    pub scope: String,
    pub storage: String,
    pub storage_posture: String,
    pub protected_storage: bool,
    pub encrypted: bool,
    pub recoverable: bool,
    pub recovery: String,
    pub uri: String,
    pub public_uri: String,
    pub profile_key: String,
    pub disk_path: String,
    pub reset: String,
}

/// An admitted Engine's declared capabilities, independent of its placement.
/// Configuration is not installed-product certification or launch readiness.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct BrowserEngineAdapterCapabilities {
    pub id: String,
    pub engine: String,
    pub default: bool,
    pub backing_substrate: String,
    pub supported_display_modes: Vec<BrowserDisplayMode>,
    pub supported_guarantee_levels: Vec<BrowserGuaranteeLevel>,
    pub network_mode: String,
    pub direct_network: bool,
    pub wallet_injection: bool,
}

impl BrowserEngineAdapterCapabilities {
    pub fn supports(&self, display: BrowserDisplayMode, guarantee: BrowserGuaranteeLevel) -> bool {
        guarantee.supports_display(display)
            && self.supported_display_modes.contains(&display)
            && self.supported_guarantee_levels.contains(&guarantee)
    }

    fn validate(&self) -> bool {
        safe_id(&self.id)
            && safe_id(&self.engine)
            && safe_id(&self.backing_substrate)
            && self.network_mode == "runtime_net_only"
            && !self.direct_network
            && !self.wallet_injection
            && !self.supported_display_modes.is_empty()
            && self.supported_display_modes.len() <= 2
            && !self.supported_guarantee_levels.is_empty()
            && self.supported_guarantee_levels.len() <= 3
            && !self
                .supported_guarantee_levels
                .contains(&BrowserGuaranteeLevel::Diagnostic)
            && self
                .supported_display_modes
                .iter()
                .map(|value| value.as_str())
                .collect::<BTreeSet<_>>()
                .len()
                == self.supported_display_modes.len()
            && self
                .supported_guarantee_levels
                .iter()
                .map(|value| value.as_str())
                .collect::<BTreeSet<_>>()
                .len()
                == self.supported_guarantee_levels.len()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BrowserEngineInventory {
    pub provider: String,
    pub protocol_version: String,
    pub status: String,
    pub adapter_count: usize,
    pub adapters: Vec<BrowserEngineAdapterCapabilities>,
    pub direct_network: bool,
    pub wallet_injection: bool,
}

pub const BROWSER_ENGINE_READINESS_SCHEMA: &str = "elastos.browser.engine-readiness/v1";

/// A host adapter's current preparation result. Capacity and permission are
/// checked separately. Private host paths stay behind the adapter boundary.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum BrowserEngineReadiness {
    Ready {},
    Unavailable {
        reason: BrowserEngineReadinessReason,
    },
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum BrowserEngineReadinessReason {
    #[error("the Engine artifacts need preparation")]
    PreparationRequired,
    #[error("the Engine artifact receipt does not match the installed files")]
    ArtifactInvalid,
    #[error("this host cannot run the selected Engine")]
    HostUnsupported,
    #[error("the Engine control service is unavailable")]
    ControlUnavailable,
    #[error("the Engine needs an update to report readiness")]
    ReadinessUnsupported,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, thiserror::Error)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum BrowserCompatibilityError {
    #[error("Browser Engine and Runtime versions are incompatible. Update them to a compatible release.")]
    IncompatibleEngineProtocol,
    #[error("Browser Engine Adapter status authority is invalid")]
    InvalidEngineStatus,
    #[error("Browser Engine is unavailable. Choose an available approved Engine.")]
    EngineUnavailable,
    #[error("The selected Browser Engine is unavailable. Choose an available approved Engine.")]
    EngineNotFound,
    #[error("Browser Engine is not ready: {reason}. Prepare this Engine or choose another approved Engine.")]
    EngineNotReady {
        reason: BrowserEngineReadinessReason,
    },
    #[error(
        "The selected Browser Engine does not support this display and isolation requirement."
    )]
    IncompatibleEngineCapabilities,
    #[error("An approved Browser Engine with the required display and isolation capabilities is needed.")]
    NoCompatibleEngine,
}

impl BrowserEngineInventory {
    pub fn from_status(data: &Value) -> Result<Self, BrowserCompatibilityError> {
        // Check the version before decoding version-specific fields. A newer
        // peer's enum values must produce a compatibility result, not a parse error.
        if data.get("provider").and_then(Value::as_str) != Some(BROWSER_ENGINE_PROVIDER_ID) {
            return Err(BrowserCompatibilityError::InvalidEngineStatus);
        }
        if data.get("protocol_version").and_then(Value::as_str)
            != Some(BROWSER_ENGINE_PROTOCOL_VERSION)
        {
            return Err(BrowserCompatibilityError::IncompatibleEngineProtocol);
        }
        if data
            .get("adapters")
            .and_then(Value::as_array)
            .is_none_or(|adapters| adapters.len() > MAX_BROWSER_ENGINE_ADAPTERS)
        {
            return Err(BrowserCompatibilityError::InvalidEngineStatus);
        }
        let inventory: Self = serde_json::from_value(data.clone())
            .map_err(|_| BrowserCompatibilityError::InvalidEngineStatus)?;
        inventory.validate()?;
        Ok(inventory)
    }

    pub fn validate(&self) -> Result<(), BrowserCompatibilityError> {
        if self.provider != BROWSER_ENGINE_PROVIDER_ID {
            return Err(BrowserCompatibilityError::InvalidEngineStatus);
        }
        if self.protocol_version != BROWSER_ENGINE_PROTOCOL_VERSION {
            return Err(BrowserCompatibilityError::IncompatibleEngineProtocol);
        }
        let mut ids = BTreeSet::new();
        let valid = !self.direct_network
            && !self.wallet_injection
            && self.adapters.len() <= MAX_BROWSER_ENGINE_ADAPTERS
            && self.adapter_count == self.adapters.len()
            && self
                .adapters
                .iter()
                .all(|adapter| adapter.validate() && ids.insert(&adapter.id))
            && match self.status.as_str() {
                "configured" => {
                    !self.adapters.is_empty()
                        && self
                            .adapters
                            .iter()
                            .filter(|adapter| adapter.default)
                            .count()
                            == 1
                }
                "unavailable" => self.adapters.is_empty(),
                _ => false,
            };
        if valid {
            Ok(())
        } else {
            Err(BrowserCompatibilityError::InvalidEngineStatus)
        }
    }

    /// Select only among the Runtime-admitted provider's capabilities. This
    /// performs no discovery, grants, launch, profile transfer or network I/O.
    pub fn select(
        &self,
        requested: Option<&str>,
        display: BrowserDisplayMode,
        guarantee: BrowserGuaranteeLevel,
    ) -> Result<&BrowserEngineAdapterCapabilities, BrowserCompatibilityError> {
        self.validate()?;
        if self.status != "configured" {
            return Err(BrowserCompatibilityError::EngineUnavailable);
        }
        if let Some(id) = requested {
            let adapter = self
                .adapters
                .iter()
                .find(|adapter| adapter.id == id)
                .ok_or(BrowserCompatibilityError::EngineNotFound)?;
            return adapter
                .supports(display, guarantee)
                .then_some(adapter)
                .ok_or(BrowserCompatibilityError::IncompatibleEngineCapabilities);
        }
        self.adapters
            .iter()
            .find(|adapter| adapter.default && adapter.supports(display, guarantee))
            .or_else(|| {
                self.adapters
                    .iter()
                    .find(|adapter| adapter.supports(display, guarantee))
            })
            .ok_or(BrowserCompatibilityError::NoCompatibleEngine)
    }
}

fn safe_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_'))
}

#[cfg(test)]
mod tests;
