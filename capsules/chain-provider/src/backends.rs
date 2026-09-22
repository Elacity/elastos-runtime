use super::*;

/// The most calls one JSON-RPC batch may carry.
///
/// Measured against the configured sources rather than chosen:
///
/// ```text
/// mainnet.base.org   40 calls -> {"error":{"code":-32014,
///                                "message":"maximum 10 calls in 1 batch"}}
/// base.drpc.org      40 calls -> 40 elements, each an error:
///                                "Batch of more than 3 requests are not
///                                 allowed on free plan"
/// ```
///
/// Three, because the strictest source decides. The two refusals are not even
/// the same shape -- one is a single object where an array was expected, the
/// other a perfectly well-formed array in which every answer is an error --
/// and the second is the dangerous one: it looks exactly like a source that
/// answered and knew nothing.
pub(super) const EVM_RPC_BATCH_MAX: usize = 3;

/// Reads a batch answer, and decides whether it is an answer at all.
///
/// Two refusals to tell apart, both seen from configured sources:
///
/// - a single object where an array was expected, which is plainly not a batch
///   answer;
/// - a well-formed array in which EVERY element carries an error, which is
///   what dRPC returns when a batch exceeds its plan. That shape is
///   indistinguishable from a source that answered and knew nothing, and read
///   as the latter it made a source contribute silence while looking healthy.
///   A whole batch failing is treated as a refusal so the caller can ask again
///   one call at a time.
///
/// An individual element carrying an error is `None` -- one reverting call
/// among several is an answer about that call, not a failure of the batch.
///
/// Answers arrive in whatever order the node chooses, so each is placed by its
/// own id and never by position.
pub(super) fn interpret_evm_rpc_batch(
    body: &Value,
    expected: usize,
) -> Result<Vec<Option<Value>>, Response> {
    let entries = body.as_array().ok_or_else(|| {
        Response::error(
            "upstream_batch_unsupported",
            "EVM RPC source did not answer a batch with a batch",
        )
    })?;
    let mut results = vec![None; expected];
    let mut errored = 0usize;
    for entry in entries {
        if entry.get("error").is_some() {
            errored += 1;
        }
        let Some(index) = entry.get("id").and_then(Value::as_u64) else {
            continue;
        };
        let Some(slot) = results.get_mut(index as usize) else {
            continue;
        };
        if entry.get("error").is_some() {
            continue;
        }
        *slot = entry.get("result").cloned();
    }
    if !entries.is_empty() && errored == entries.len() {
        return Err(Response::error(
            "upstream_batch_refused",
            "EVM RPC source answered every call in the batch with an error",
        ));
    }
    Ok(results)
}

pub(super) struct MainchainTip {
    pub(super) height: u64,
    pub(super) hash: String,
    pub(super) timestamp: Option<u64>,
    pub(super) tx_count: Option<u64>,
}

impl ChainProvider {
    pub(super) fn network_for_status(&self, network_id: &str) -> Result<&ChainNetwork, Response> {
        if let Err(err) = validate_network_id(network_id) {
            return Err(Response::error("invalid_network", &err));
        }
        self.networks
            .iter()
            .find(|network| network.id == network_id)
            .ok_or_else(|| Response::error("unknown_network", "unknown chain network"))
    }

    pub(super) fn evm_network(&self, network_id: &str) -> Result<&ChainNetwork, Response> {
        let network = self.network_for_status(network_id)?;
        if network.kind != ChainKind::EvmJsonRpc {
            return Err(Response::error(
                "unsupported_network_kind",
                "this operation currently supports EVM JSON-RPC networks only",
            ));
        }
        Ok(network)
    }

    pub(super) fn bitcoin_status(&self, network: &ChainNetwork) -> Response {
        let info = match self.bitcoin_rpc(network, "getblockchaininfo", json!([])) {
            Ok(value) => value,
            Err(response) => return response,
        };
        Response::ok(json!({
            "network": network.public_view(),
            "chain": info.get("chain").cloned().unwrap_or(Value::Null),
            "block_height": info.get("blocks").and_then(Value::as_u64),
            "headers": info.get("headers").and_then(Value::as_u64),
            "best_block_hash": info.get("bestblockhash").cloned().unwrap_or(Value::Null),
            "initial_block_download": info.get("initialblockdownload").and_then(Value::as_bool),
            "verification_progress": info.get("verificationprogress").and_then(Value::as_f64),
        }))
    }

    pub(super) fn bitcoin_rest_status(&self, network: &ChainNetwork) -> Response {
        let block_height = match self.bitcoin_rest_tip_height(network) {
            Ok(height) => height,
            Err(response) => return response,
        };
        let best_block_hash = match self.backend_get_text(network, "blocks/tip/hash") {
            Ok(hash) => hash.trim().to_string(),
            Err(response) => return response,
        };
        Response::ok(json!({
            "network": network.public_view(),
            "chain": "main",
            "block_height": block_height,
            "best_block_hash": best_block_hash,
        }))
    }

    pub(super) fn bitcoin_rest_tip_height(&self, network: &ChainNetwork) -> Result<u64, Response> {
        let text = self.backend_get_text(network, "blocks/tip/height")?;
        text.trim()
            .parse::<u64>()
            .map_err(|err| Response::error("upstream_invalid_height", &err.to_string()))
    }

    pub(super) fn mainchain_status(&self, network: &ChainNetwork) -> Response {
        let tip = match self.mainchain_tip(network) {
            Ok(tip) => tip,
            Err(response) => return response,
        };
        Response::ok(json!({
            "network": network.public_view(),
            "block_height": tip.height,
            "best_block_hash": tip.hash,
            "timestamp": tip.timestamp,
            "tx_count": tip.tx_count,
        }))
    }

    pub(super) fn mainchain_tip(&self, network: &ChainNetwork) -> Result<MainchainTip, Response> {
        let body = self.backend_get_json(network, "blocks?page=1&pageSize=1")?;
        let block = body
            .get("data")
            .and_then(Value::as_array)
            .and_then(|blocks| blocks.first())
            .ok_or_else(|| {
                Response::error("upstream_missing_result", "mainchain tip block missing")
            })?;
        let height = block.get("height").and_then(Value::as_u64).ok_or_else(|| {
            Response::error("upstream_invalid_height", "mainchain block height missing")
        })?;
        let hash = block
            .get("hash")
            .and_then(Value::as_str)
            .filter(|hash| !hash.trim().is_empty())
            .ok_or_else(|| {
                Response::error("upstream_invalid_hash", "mainchain block hash missing")
            })?
            .to_string();
        Ok(MainchainTip {
            height,
            hash,
            timestamp: block.get("timestamp").and_then(Value::as_u64),
            tx_count: block.get("txCount").and_then(Value::as_u64),
        })
    }

    pub(super) fn backend_get_json(
        &self,
        network: &ChainNetwork,
        path: &str,
    ) -> Result<Value, Response> {
        let url = backend_url(network, path)?;
        let response = self
            .client
            .get(url)
            .send()
            .map_err(|err| Response::error("upstream_unreachable", &err.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(Response::error(
                "upstream_http_error",
                &format!("upstream returned HTTP {}", status.as_u16()),
            ));
        }
        response
            .json::<Value>()
            .map_err(|err| Response::error("upstream_invalid_json", &err.to_string()))
    }

    pub(super) fn backend_get_text(
        &self,
        network: &ChainNetwork,
        path: &str,
    ) -> Result<String, Response> {
        let url = backend_url(network, path)?;
        let response = self
            .client
            .get(url)
            .send()
            .map_err(|err| Response::error("upstream_unreachable", &err.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(Response::error(
                "upstream_http_error",
                &format!("upstream returned HTTP {}", status.as_u16()),
            ));
        }
        response
            .text()
            .map_err(|err| Response::error("upstream_invalid_text", &err.to_string()))
    }

    pub(super) fn evm_rpc(
        &self,
        network: &ChainNetwork,
        method: &str,
        params: Value,
    ) -> Result<Value, Response> {
        let response = self
            .client
            .post(&network.rpc_url)
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": method,
                "params": params,
            }))
            .send()
            .map_err(|_| Response::error("upstream_unreachable", "EVM RPC request failed"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(Response::error(
                "upstream_http_error",
                &format!("upstream returned HTTP {}", status.as_u16()),
            ));
        }
        let body = response
            .json::<Value>()
            .map_err(|_| Response::error("upstream_invalid_json", "EVM RPC response malformed"))?;
        if let Some(error) = body.get("error") {
            // Keep the node's own verdict (code, message and, for reverts,
            // the ABI-encoded error data) so a caller can tell a reverted
            // estimate from a rejected transaction without the RPC console.
            let code = error.get("code").cloned().unwrap_or(Value::Null);
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("EVM RPC request rejected");
            let data = error
                .get("data")
                .map(|data| data.to_string())
                .unwrap_or_default();
            let mut detail = format!("EVM RPC request rejected: {method}: code {code}: {message}");
            if !data.is_empty() && data != "null" {
                detail.push_str(&format!(": data {data}"));
            }
            detail.truncate(1024);
            return Err(Response::error("upstream_rpc_error", &detail));
        }
        body.get("result")
            .cloned()
            .ok_or_else(|| Response::error("upstream_missing_result", "RPC result missing"))
    }

    /// Several `eth_call`s to one source in a single JSON-RPC batch.
    ///
    /// The Shops surface asks two questions of every channel across every
    /// configured source. Sent one at a time that is eighty-odd round trips
    /// for a directory of fourteen channels, which measured 46 seconds on an
    /// installed Home while every card sat at "checking".
    ///
    /// Answers come back in whatever order the node chooses, so each is
    /// matched by its own id and never by position. An element that carries an
    /// `error`, or that no answer arrives for, is `None` -- the caller decides
    /// what a missing answer means, and for an access check it means "unknown"
    /// rather than "no".
    ///
    /// A node that refuses batches is reported as one failure rather than
    /// silently as a set of them; the caller falls back to asking singly.
    pub(super) fn evm_rpc_batch(
        &self,
        network: &ChainNetwork,
        calls: &[(String, Value)],
    ) -> Result<Vec<Option<Value>>, Response> {
        if calls.is_empty() {
            return Ok(Vec::new());
        }
        if calls.len() > EVM_RPC_BATCH_MAX {
            return Err(Response::error(
                "invalid_batch_request",
                "too many calls in one EVM RPC batch",
            ));
        }
        let payload: Vec<Value> = calls
            .iter()
            .enumerate()
            .map(|(index, (method, params))| {
                json!({
                    "jsonrpc": "2.0",
                    "id": index,
                    "method": method,
                    "params": params,
                })
            })
            .collect();
        let response = self
            .client
            .post(&network.rpc_url)
            .json(&payload)
            .send()
            .map_err(|_| Response::error("upstream_unreachable", "EVM RPC batch request failed"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(Response::error(
                "upstream_http_error",
                &format!("upstream returned HTTP {}", status.as_u16()),
            ));
        }
        let body = response
            .json::<Value>()
            .map_err(|_| Response::error("upstream_invalid_json", "EVM RPC batch malformed"))?;
        interpret_evm_rpc_batch(&body, calls.len())
    }

    pub(super) fn bitcoin_rpc(
        &self,
        network: &ChainNetwork,
        method: &str,
        params: Value,
    ) -> Result<Value, Response> {
        if network.rpc_url.trim().is_empty() {
            return Err(Response::error(
                "node_not_configured",
                "Bitcoin Core RPC is not configured for this network",
            ));
        }
        let mut request = self.client.post(&network.rpc_url).json(&json!({
            "jsonrpc": "1.0",
            "id": "elastos-chain-provider",
            "method": method,
            "params": params,
        }));
        if let Some((user, password)) = bitcoin_rpc_auth(&network.id) {
            request = request.basic_auth(user, Some(password));
        }
        let response = request
            .send()
            .map_err(|err| Response::error("upstream_unreachable", &err.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(Response::error(
                "upstream_http_error",
                &format!("upstream returned HTTP {}", status.as_u16()),
            ));
        }
        let body = response
            .json::<Value>()
            .map_err(|err| Response::error("upstream_invalid_json", &err.to_string()))?;
        if let Some(error) = body.get("error").filter(|value| !value.is_null()) {
            return Err(Response::error("upstream_rpc_error", &error.to_string()));
        }
        body.get("result")
            .cloned()
            .ok_or_else(|| Response::error("upstream_missing_result", "RPC result missing"))
    }
}
