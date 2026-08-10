//! `elastos.model.offer/v1` — model capability descriptor.
//!
//! Served by `offers.list`. The descriptor carries capability metadata only:
//! no upstream URLs, ports, or credentials ever appear in an offer (SSRF-closed,
//! SP-CRYPTO). Wrapping into `elastos.service.offer/v1` for DID/Carrier
//! discovery happens at publish time, not here.

use serde::Serialize;

pub const MODEL_OFFER_SCHEMA: &str = "elastos.model.offer/v1";

#[derive(Debug, Clone, Serialize)]
pub struct ModelOffer {
    pub schema: &'static str,
    pub offer_id: String,
    pub provider_profile_did: String,
    pub service_offer_id: String,
    pub model: ModelInfo,
    pub operations: Vec<Operation>,
    pub policy: Policy,
    pub terms_ref: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub revision: String,
    pub digest: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Operation {
    pub id: String,
    pub inputs: Vec<IoSpec>,
    pub outputs: Vec<IoSpec>,
    pub features: Features,
    pub parameters_schema: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct IoSpec {
    pub name: String,
    pub modalities: Vec<String>,
    /// Present on inputs (true/false), omitted on outputs — matches the
    /// published mapping doc byte-for-byte.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    pub delivery: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Features {
    pub stream_input: bool,
    pub stream_output: bool,
    pub progress: bool,
    pub cancel: bool,
    pub additional_input: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Policy {
    pub maximum_concurrent_runs: u32,
    pub maximum_input_bytes: u64,
    pub maximum_run_seconds: u64,
    pub data_retention_seconds: u64,
    pub training_use: bool,
}

/// Operator-owned config from Init (`config.extra`). Values shape the offers;
/// upstream URLs stay internal to the provider for run dispatch (P2/P3).
#[derive(Debug, Clone, Default)]
pub struct ProviderConfig {
    pub flash_url: Option<String>,
    pub h3_url: Option<String>,
    pub flash_digest: Option<String>,
    pub h3_digest: Option<String>,
    pub provider_profile_did: Option<String>,
    /// Directory for durable run artifacts. `extra.h3_output_dir` wins;
    /// falls back to `<base_path>/creative/jobs` (the dogfood library
    /// convention), then `./creative/jobs`.
    pub base_path: Option<String>,
    pub h3_output_dir: Option<String>,
}

impl ProviderConfig {
    pub fn output_dir(&self) -> std::path::PathBuf {
        if let Some(dir) = &self.h3_output_dir {
            return std::path::PathBuf::from(dir);
        }
        if let Some(base) = &self.base_path {
            if !base.is_empty() {
                return std::path::PathBuf::from(base).join("creative").join("jobs");
            }
        }
        std::path::PathBuf::from("creative/jobs")
    }
}

impl ProviderConfig {
    pub fn from_init(config: &serde_json::Value) -> Self {
        let extra = config.get("extra").unwrap_or(config);
        let get = |key: &str| {
            extra
                .get(key)
                .and_then(|value| value.as_str())
                .map(str::to_string)
        };
        let base_path = config
            .get("base_path")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        ProviderConfig {
            flash_url: get("flash_url"),
            h3_url: get("h3_url"),
            flash_digest: get("flash_digest"),
            h3_digest: get("h3_digest"),
            provider_profile_did: get("provider_profile_did"),
            base_path,
            h3_output_dir: get("h3_output_dir"),
        }
    }
}

pub fn flash_chat_offer(config: &ProviderConfig) -> ModelOffer {
    ModelOffer {
        schema: MODEL_OFFER_SCHEMA,
        offer_id: "offer:flash-chat:pair-a".to_string(),
        provider_profile_did: config
            .provider_profile_did
            .clone()
            .unwrap_or_else(|| "did:key:sparks-flash".to_string()),
        service_offer_id: "service:model:chat".to_string(),
        model: ModelInfo {
            id: "deepseek-v4-flash".to_string(),
            revision: "2026-08-10".to_string(),
            digest: config
                .flash_digest
                .clone()
                .unwrap_or_else(|| "sha256:<model-digest>".to_string()),
            name: "DeepSeek V4 Flash (Sparks pair A)".to_string(),
        },
        operations: vec![Operation {
            id: "generate".to_string(),
            inputs: vec![IoSpec {
                name: "messages".to_string(),
                modalities: vec!["text".to_string()],
                required: Some(true),
                delivery: vec!["inline".to_string()],
            }],
            outputs: vec![IoSpec {
                name: "text".to_string(),
                modalities: vec!["text".to_string()],
                required: None,
                delivery: vec!["inline".to_string()],
            }],
            features: Features {
                stream_input: false,
                stream_output: true,
                progress: false,
                cancel: true,
                additional_input: true,
            },
            parameters_schema: serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "max_tokens": { "type": "integer", "minimum": 1, "maximum": 8192 },
                    "temperature": { "type": "number", "minimum": 0, "maximum": 2 }
                }
            }),
        }],
        policy: Policy {
            maximum_concurrent_runs: 8,
            maximum_input_bytes: 1_048_576,
            maximum_run_seconds: 120,
            data_retention_seconds: 0,
            training_use: false,
        },
        terms_ref: "elastos://services/terms/sparks-flash".to_string(),
    }
}

pub fn h3_video_offer(config: &ProviderConfig) -> ModelOffer {
    ModelOffer {
        schema: MODEL_OFFER_SCHEMA,
        offer_id: "offer:h3-video:2x".to_string(),
        provider_profile_did: config
            .provider_profile_did
            .clone()
            .unwrap_or_else(|| "did:key:sparks-h3".to_string()),
        service_offer_id: "service:model:video".to_string(),
        model: ModelInfo {
            id: "minimax-h3-fl2va".to_string(),
            revision: "2026-08-09".to_string(),
            digest: config
                .h3_digest
                .clone()
                .unwrap_or_else(|| "sha256:<model-digest>".to_string()),
            name: "MiniMax H3 FL2VA (Sparks 2×)".to_string(),
        },
        operations: vec![Operation {
            id: "generate".to_string(),
            inputs: vec![
                IoSpec {
                    name: "prompt".to_string(),
                    modalities: vec!["text".to_string()],
                    required: Some(true),
                    delivery: vec!["inline".to_string()],
                },
                IoSpec {
                    name: "reference".to_string(),
                    modalities: vec![
                        "image".to_string(),
                        "video".to_string(),
                        "audio".to_string(),
                    ],
                    required: Some(false),
                    delivery: vec!["object".to_string()],
                },
            ],
            outputs: vec![IoSpec {
                name: "video".to_string(),
                modalities: vec!["video".to_string()],
                required: None,
                delivery: vec!["object".to_string()],
            }],
            features: Features {
                stream_input: false,
                stream_output: false,
                progress: true,
                cancel: true,
                additional_input: false,
            },
            parameters_schema: serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "duration_seconds": { "type": "integer", "minimum": 1, "maximum": 30 },
                    "resolution": { "enum": ["720p", "1080p"] },
                    "aspect_ratio": { "enum": ["16:9", "9:16", "1:1"] },
                    "scale": { "enum": [1, 2, 4], "default": 2 }
                }
            }),
        }],
        policy: Policy {
            maximum_concurrent_runs: 1,
            maximum_input_bytes: 104_857_600,
            maximum_run_seconds: 3000,
            data_retention_seconds: 0,
            training_use: false,
        },
        terms_ref: "elastos://services/terms/h3-video".to_string(),
    }
}

/// All offers configured on this provider. An offer is only advertised when
/// its backend is configured — honesty about readiness (never advertise what
/// we cannot serve).
pub fn configured_offers(config: &ProviderConfig) -> Vec<ModelOffer> {
    let mut offers = Vec::new();
    if config.flash_url.is_some() {
        offers.push(flash_chat_offer(config));
    }
    if config.h3_url.is_some() {
        offers.push(h3_video_offer(config));
    }
    if offers.is_empty() {
        offers.push(flash_chat_offer(config));
    }
    offers
}
