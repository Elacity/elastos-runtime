//! Browser wire types shared by Runtime and its Engine provider.
//!
//! These types describe capabilities and page semantics. Host detection, service
//! grants, installation and transport remain with Runtime and host adapters.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

mod inspection;
pub use inspection::*;
mod operator;
pub use operator::*;

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
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub display_generation: Option<String>,
}

pub const BROWSER_DISPLAY_ATTACH_REQUEST_SCHEMA: &str = "elastos.browser.display-attach-request/v1";
pub const BROWSER_DISPLAY_ATTACH_RESULT_SCHEMA: &str = "elastos.browser.display-attach-result/v1";

pub fn browser_display_request_id_valid(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub fn browser_display_generation_valid(value: &str) -> bool {
    value
        .strip_prefix("display:")
        .is_some_and(browser_display_request_id_valid)
}

/// Fixed public errors: private control response bodies never become UI diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserDisplayError {
    Busy,
    GenerationMismatch,
    OwnerChanged,
    Unsupported,
    Failed,
    Uncertain,
}

impl BrowserDisplayError {
    pub fn code(self) -> &'static str {
        match self {
            Self::Busy => "display_attach_busy",
            Self::GenerationMismatch => "display_generation_mismatch",
            Self::OwnerChanged => "display_owner_changed",
            Self::Unsupported => "display_attach_unsupported",
            Self::Failed => "display_attach_failed",
            Self::Uncertain => "display_attach_uncertain",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        [
            Self::Busy,
            Self::GenerationMismatch,
            Self::OwnerChanged,
            Self::Unsupported,
            Self::Failed,
            Self::Uncertain,
        ]
        .into_iter()
        .find(|error| error.code() == code)
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::Busy => "Browser display attachment is already pending.",
            Self::GenerationMismatch => "Browser display generation changed.",
            Self::OwnerChanged => "Browser page ownership changed during display attachment.",
            Self::Unsupported => {
                "Browser Engine does not support display attachment for this page."
            }
            Self::Failed => "Browser display attachment failed. The page remains owned by Runtime.",
            Self::Uncertain => {
                "Browser display attachment outcome is uncertain. Retry the same request."
            }
        }
    }

    pub fn http_status(self) -> u16 {
        match self {
            Self::Busy | Self::GenerationMismatch | Self::OwnerChanged => 409,
            Self::Unsupported => 501,
            Self::Failed | Self::Uncertain => 503,
        }
    }
}

pub fn validate_browser_display_attach_result(
    result: &Value,
    page_id: &str,
    request_id: &str,
    previous_generation: &str,
) -> Result<(), BrowserDisplayError> {
    let keys = [
        "schema",
        "page_id",
        "request_id",
        "previous_display_generation",
        "display_generation",
        "initial_offer",
        "audio_offer",
    ];
    let valid_candidate = |candidate: &Value| {
        candidate.as_object().is_some_and(|object| {
            object.keys().all(|key| {
                matches!(
                    key.as_str(),
                    "candidate" | "sdpMid" | "sdpMLineIndex" | "usernameFragment"
                )
            })
        }) && candidate
            .get("candidate")
            .and_then(Value::as_str)
            .is_some_and(|line| !line.trim().is_empty() && line.len() <= 32 * 1024)
            && candidate.get("sdpMid").is_none_or(|mid| {
                mid.is_null() || mid.as_str().is_some_and(|value| value.len() <= 256)
            })
            && candidate.get("sdpMLineIndex").is_none_or(|index| {
                index.is_null() || index.as_u64().is_some_and(|value| value <= 65535)
            })
            && candidate.get("usernameFragment").is_none_or(|fragment| {
                fragment.is_null() || fragment.as_str().is_some_and(|value| value.len() <= 256)
            })
    };
    let valid_offer = |offer: &Value| {
        offer.get("schema").and_then(Value::as_str) == Some("elastos.browser.webrtc-offer/v1")
            && offer.get("type").and_then(Value::as_str) == Some("offer")
            && offer
                .get("sdp")
                .and_then(Value::as_str)
                .is_some_and(|sdp| !sdp.trim().is_empty() && sdp.len() <= 256 * 1024)
            && offer.as_object().is_some_and(|object| {
                object.keys().all(|key| {
                    matches!(
                        key.as_str(),
                        "schema" | "type" | "sdp" | "candidates" | "end_of_candidates"
                    )
                })
            })
            && offer.get("candidates").is_none_or(|candidates| {
                candidates
                    .as_array()
                    .is_some_and(|values| values.len() <= 256 && values.iter().all(valid_candidate))
            })
            && offer.get("end_of_candidates").is_none_or(Value::is_boolean)
    };
    if result.as_object().is_none_or(|object| {
        object.len() != keys.len() || keys.iter().any(|key| !object.contains_key(*key))
    }) || result.get("schema").and_then(Value::as_str)
        != Some(BROWSER_DISPLAY_ATTACH_RESULT_SCHEMA)
        || result.get("page_id").and_then(Value::as_str) != Some(page_id)
        || result.get("request_id").and_then(Value::as_str) != Some(request_id)
        || result
            .get("previous_display_generation")
            .and_then(Value::as_str)
            != Some(previous_generation)
        || !result
            .get("display_generation")
            .and_then(Value::as_str)
            .is_some_and(|generation| {
                browser_display_generation_valid(generation) && generation != previous_generation
            })
        || !valid_offer(&result["initial_offer"])
        || !valid_offer(&result["audio_offer"])
        || serde_json::to_vec(result).map_or(true, |bytes| bytes.len() > 1024 * 1024)
    {
        return Err(BrowserDisplayError::Uncertain);
    }
    Ok(())
}

/// One live display and one retained attach attempt per owned page. This state is
/// deliberately ephemeral; restarting Runtime does not grant a new attachment.
#[derive(Debug, Clone, Default)]
pub struct BrowserDisplayAttachment {
    generation: Option<String>,
    attempt: Option<BrowserDisplayAttachAttempt>,
}

#[derive(Debug, Clone)]
struct BrowserDisplayAttachAttempt {
    request_id: String,
    previous_generation: String,
    outcome: Option<Result<Value, BrowserDisplayError>>,
}

impl BrowserDisplayAttachment {
    pub fn initial(generation: Option<String>) -> Self {
        Self {
            generation,
            attempt: None,
        }
    }

    pub fn summary(&self) -> Option<Value> {
        let attempt = self.attempt.as_ref()?;
        let mut summary = serde_json::json!({
            "schema": "elastos.browser.display-attachment/v1",
            "state": match &attempt.outcome { None => "pending", Some(Ok(_)) => "ready", Some(Err(_)) => "failed" },
            "request_id": attempt.request_id,
            "previous_display_generation": attempt.previous_generation,
        });
        if let Some(Err(error)) = &attempt.outcome {
            summary["error_code"] = serde_json::json!(error.code());
        }
        Some(summary)
    }

    pub fn check_signal(&self, generation: Option<&str>) -> Result<(), BrowserDisplayError> {
        if let Some(attempt) = &self.attempt {
            if attempt.outcome.is_none() {
                return Err(BrowserDisplayError::Busy);
            }
            if !matches!(attempt.outcome, Some(Ok(_))) || generation.is_none() {
                return Err(BrowserDisplayError::GenerationMismatch);
            }
        }
        if generation.is_some() && generation != self.generation.as_deref() {
            return Err(BrowserDisplayError::GenerationMismatch);
        }
        Ok(())
    }

    /// None means dispatch (or reconcile the same uncertain request); Some is a
    /// cached successful result. The peer must coalesce concurrent same-ID calls.
    pub fn begin(
        &mut self,
        request_id: &str,
        previous_generation: &str,
    ) -> Result<Option<Value>, BrowserDisplayError> {
        if !browser_display_request_id_valid(request_id)
            || !browser_display_generation_valid(previous_generation)
        {
            return Err(BrowserDisplayError::GenerationMismatch);
        }
        if self.generation.is_none() {
            return Err(BrowserDisplayError::Unsupported);
        }
        if let Some(attempt) = &self.attempt {
            if attempt.request_id == request_id {
                if attempt.previous_generation != previous_generation {
                    return Err(BrowserDisplayError::GenerationMismatch);
                }
                return attempt.outcome.clone().transpose();
            }
            if attempt.outcome.is_none() {
                return Err(BrowserDisplayError::Busy);
            }
        }
        if self.generation.as_deref() != Some(previous_generation) {
            return Err(BrowserDisplayError::GenerationMismatch);
        }
        self.attempt = Some(BrowserDisplayAttachAttempt {
            request_id: request_id.to_string(),
            previous_generation: previous_generation.to_string(),
            outcome: None,
        });
        Ok(None)
    }

    pub fn finish(
        &mut self,
        page_id: &str,
        request_id: &str,
        previous_generation: &str,
        outcome: Result<Value, BrowserDisplayError>,
    ) -> Result<Value, BrowserDisplayError> {
        let attempt = self
            .attempt
            .as_mut()
            .filter(|attempt| {
                attempt.request_id == request_id
                    && attempt.previous_generation == previous_generation
            })
            .ok_or(BrowserDisplayError::OwnerChanged)?;
        if let Some(cached) = &attempt.outcome {
            return cached.clone();
        }
        let outcome = outcome.and_then(|value| {
            validate_browser_display_attach_result(
                &value,
                page_id,
                request_id,
                previous_generation,
            )?;
            Ok(value)
        });
        if let Ok(result) = &outcome {
            self.generation = result
                .get("display_generation")
                .and_then(Value::as_str)
                .map(str::to_string);
        }
        // A busy or uncertain peer may already own effects. Only the same request
        // may reconcile them; another ID cannot silently reset the display.
        if !matches!(
            outcome,
            Err(BrowserDisplayError::Busy | BrowserDisplayError::Uncertain)
        ) {
            attempt.outcome = Some(outcome.clone());
        }
        outcome
    }
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

#[cfg(test)]
mod display_attach_tests {
    use super::*;
    use serde_json::json;

    fn generation(letter: char) -> String {
        format!("display:{}", letter.to_string().repeat(32))
    }
    fn request(letter: char) -> String {
        letter.to_string().repeat(32)
    }
    fn result() -> Value {
        json!({"schema": BROWSER_DISPLAY_ATTACH_RESULT_SCHEMA, "page_id": "page:test",
            "request_id": request('a'), "previous_display_generation": generation('a'), "display_generation": generation('b'),
            "initial_offer": {"schema":"elastos.browser.webrtc-offer/v1","type":"offer","sdp":"v=0\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\n","candidates":[]},
            "audio_offer": {"schema":"elastos.browser.webrtc-offer/v1","type":"offer","sdp":"v=0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\n","candidates":[]}})
    }

    #[test]
    fn display_attach_legacy_is_optional_until_first_attach() {
        let mut legacy = BrowserDisplayAttachment::default();
        assert_eq!(legacy.check_signal(None), Ok(()));
        assert_eq!(
            legacy.begin(&request('a'), &generation('a')),
            Err(BrowserDisplayError::Unsupported)
        );
        assert_eq!(legacy.check_signal(None), Ok(()));
        let mut current = BrowserDisplayAttachment::initial(Some(generation('a')));
        assert_eq!(current.check_signal(None), Ok(()));
        assert_eq!(current.check_signal(Some(&generation('a'))), Ok(()));
        assert_eq!(
            current.check_signal(Some(&generation('b'))),
            Err(BrowserDisplayError::GenerationMismatch)
        );
        current.begin(&request('a'), &generation('a')).unwrap();
        assert_eq!(current.check_signal(None), Err(BrowserDisplayError::Busy));
        assert_eq!(
            current.check_signal(Some(&generation('a'))),
            Err(BrowserDisplayError::Busy)
        );
        current
            .finish("page:test", &request('a'), &generation('a'), Ok(result()))
            .unwrap();
        assert_eq!(
            current.check_signal(None),
            Err(BrowserDisplayError::GenerationMismatch)
        );
        assert_eq!(
            current.check_signal(Some(&generation('a'))),
            Err(BrowserDisplayError::GenerationMismatch)
        );
        assert_eq!(current.check_signal(Some(&generation('b'))), Ok(()));
    }

    #[test]
    fn display_attach_uncertain_same_id_reconciles_and_replay_precedes_generation_cas() {
        let mut current = BrowserDisplayAttachment::initial(Some(generation('a')));
        assert_eq!(current.begin(&request('a'), &generation('a')), Ok(None));
        assert_eq!(current.begin(&request('a'), &generation('a')), Ok(None));
        assert_eq!(
            current.begin(&request('b'), &generation('a')),
            Err(BrowserDisplayError::Busy)
        );
        assert_eq!(
            current.finish(
                "page:test",
                &request('a'),
                &generation('a'),
                Err(BrowserDisplayError::Uncertain)
            ),
            Err(BrowserDisplayError::Uncertain)
        );
        assert_eq!(
            current.begin(&request('b'), &generation('a')),
            Err(BrowserDisplayError::Busy)
        );
        assert_eq!(current.begin(&request('a'), &generation('a')), Ok(None));
        let receipt = current
            .finish("page:test", &request('a'), &generation('a'), Ok(result()))
            .unwrap();
        assert_eq!(
            current.begin(&request('a'), &generation('a')),
            Ok(Some(receipt.clone()))
        );
        assert_eq!(
            current.finish(
                "page:test",
                &request('a'),
                &generation('a'),
                Err(BrowserDisplayError::Failed)
            ),
            Ok(receipt)
        );
        assert_eq!(
            current.begin(&request('a'), &generation('b')),
            Err(BrowserDisplayError::GenerationMismatch)
        );
        assert_eq!(
            current.begin(&request('b'), &generation('a')),
            Err(BrowserDisplayError::GenerationMismatch)
        );
        assert_eq!(current.begin(&request('b'), &generation('b')), Ok(None));
        assert_eq!(
            current.finish("page:test", &request('a'), &generation('a'), Ok(result())),
            Err(BrowserDisplayError::OwnerChanged)
        );
    }

    #[test]
    fn display_attach_failed_receipt_disables_old_peer_but_allows_explicit_new_attempt() {
        let mut current = BrowserDisplayAttachment::initial(Some(generation('a')));
        current.begin(&request('a'), &generation('a')).unwrap();
        assert_eq!(
            current.finish(
                "page:test",
                &request('a'),
                &generation('a'),
                Err(BrowserDisplayError::Failed)
            ),
            Err(BrowserDisplayError::Failed)
        );
        assert_eq!(
            current.begin(&request('a'), &generation('a')),
            Err(BrowserDisplayError::Failed)
        );
        assert_eq!(
            current.check_signal(Some(&generation('a'))),
            Err(BrowserDisplayError::GenerationMismatch)
        );
        assert_eq!(current.begin(&request('b'), &generation('a')), Ok(None));
    }

    #[test]
    fn display_attach_result_rejects_wrong_bindings_and_authority_substitution() {
        for (key, value) in [
            ("page_id", json!("page:foreign")),
            ("request_id", json!(request('b'))),
            ("previous_display_generation", json!(generation('c'))),
            ("display_generation", json!(generation('a'))),
            ("runtime_turn", json!({"credential":"private"})),
            ("display_session", json!({"mode":"native_surface"})),
        ] {
            let mut bad = result();
            bad[key] = value;
            assert_eq!(
                validate_browser_display_attach_result(
                    &bad,
                    "page:test",
                    &request('a'),
                    &generation('a')
                ),
                Err(BrowserDisplayError::Uncertain)
            );
        }
        let mut current = BrowserDisplayAttachment::initial(Some(generation('a')));
        current.begin(&request('a'), &generation('a')).unwrap();
        let mut bad = result();
        bad["initial_offer"]["credential"] = json!("private");
        assert_eq!(
            current.finish("page:test", &request('a'), &generation('a'), Ok(bad)),
            Err(BrowserDisplayError::Uncertain)
        );
        assert_eq!(
            current.begin(&request('b'), &generation('a')),
            Err(BrowserDisplayError::Busy)
        );
    }
}
