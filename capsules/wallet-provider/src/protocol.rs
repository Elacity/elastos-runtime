use super::*;

fn deserialize_required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Request {
    Init {
        #[serde(default)]
        config: Value,
    },
    Status,
    WalletContract {
        request: Value,
        #[serde(rename = "_runtime_invocation")]
        runtime_invocation: Box<RuntimeInvocationEnvelope>,
    },
    Shutdown,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RuntimeInvocationEnvelope {
    schema: String,
    source: String,
    target: String,
    op: String,
    capability: String,
    transport: String,
    #[serde(deserialize_with = "deserialize_required_option")]
    carrier: Option<Value>,
    transfer: String,
    #[serde(deserialize_with = "deserialize_required_option")]
    range: Option<Value>,
    #[serde(deserialize_with = "deserialize_required_option")]
    progress: Option<Value>,
    abi: RuntimeInvocationAbi,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeInvocationAbi {
    schema: String,
    transfer: String,
    transport: String,
    range_supported: bool,
    progress_supported: bool,
    progress_mode: String,
    transport_native_stream: bool,
    backpressure: String,
    cancel_supported: bool,
}

impl RuntimeInvocationEnvelope {
    pub(super) fn validate_wallet_contract(&self) -> Result<(), String> {
        let exact = [
            (
                "schema",
                self.schema.as_str(),
                "elastos.provider.invocation/v1",
            ),
            ("source", self.source.as_str(), "runtime"),
            ("target", self.target.as_str(), "wallet"),
            ("op", self.op.as_str(), WALLET_BUS_OPERATION),
            (
                "capability",
                self.capability.as_str(),
                "provider:runtime->wallet:wallet_contract",
            ),
            (
                "transport",
                self.transport.as_str(),
                "runtime-local-provider-plane",
            ),
            ("transfer", self.transfer.as_str(), "json"),
        ];
        for (field, actual, expected) in exact {
            if actual != expected {
                return Err(format!(
                    "Wallet contract requires {field}={expected}, received {actual}"
                ));
            }
        }
        if self.carrier.is_some() || self.range.is_some() || self.progress.is_some() {
            return Err(
                "Wallet contract forbids Carrier, range, and progress invocation metadata"
                    .to_string(),
            );
        }
        let abi = &self.abi;
        if abi.schema != "elastos.provider.transfer-abi/v1"
            || abi.transfer != "json"
            || abi.transport != "runtime-local-provider-plane"
            || abi.range_supported
            || abi.progress_supported
            || abi.progress_mode != "none"
            || abi.transport_native_stream
            || abi.backpressure != "not_applicable"
            || abi.cancel_supported
        {
            return Err("Wallet contract requires the exact Runtime-local JSON ABI".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Response {
    Ok {
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<Value>,
    },
    Error {
        code: String,
        message: String,
    },
}

impl Response {
    pub(super) fn ok(data: Value) -> Self {
        Self::Ok { data: Some(data) }
    }

    pub(super) fn empty_ok() -> Self {
        Self::Ok { data: None }
    }

    pub(super) fn error(code: &str, message: impl Into<String>) -> Self {
        Self::Error {
            code: code.to_string(),
            message: message.into(),
        }
    }

    pub(super) fn is_ok(&self) -> bool {
        matches!(self, Self::Ok { .. })
    }
}
