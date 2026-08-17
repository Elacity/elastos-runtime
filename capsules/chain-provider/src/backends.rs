use super::*;

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

    /// The ordered RPC endpoints for a network: the primary `rpc_url` first, then each
    /// `rpc_fallback_urls` entry, de-duplicated and trimmed of empties. Mirrors PC2's
    /// round-robin pool ordering (primary head, public fallbacks behind).
    fn evm_rpc_pool<'a>(network: &'a ChainNetwork) -> Vec<&'a str> {
        let mut pool: Vec<&str> = Vec::new();
        for url in std::iter::once(&network.rpc_url).chain(network.rpc_fallback_urls.iter()) {
            let url = url.trim();
            if !url.is_empty() && !pool.contains(&url) {
                pool.push(url);
            }
        }
        pool
    }

    /// The RPC pool for `eth_getLogs`: the network's `log_query_rpc_urls` subset (range-capable
    /// endpoints) when configured, else the full pool. Channel discovery routes log queries
    /// here so a strict public endpoint (e.g. `1rpc.io`'s 50-block cap) can never break a scan.
    fn evm_log_rpc_pool<'a>(network: &'a ChainNetwork) -> Vec<&'a str> {
        let mut pool: Vec<&str> = Vec::new();
        for url in &network.log_query_rpc_urls {
            let url = url.trim();
            if !url.is_empty() && !pool.contains(&url) {
                pool.push(url);
            }
        }
        if pool.is_empty() {
            return Self::evm_rpc_pool(network);
        }
        pool
    }

    /// `eth_getLogs` over the range-capable pool only. Same rotate-on-error failover as
    /// `evm_rpc`, but restricted to endpoints that actually serve wide log ranges.
    pub(super) fn evm_rpc_logs(
        &self,
        network: &ChainNetwork,
        filter: Value,
    ) -> Result<Value, Response> {
        let pool = Self::evm_log_rpc_pool(network);
        if pool.is_empty() {
            return Err(Response::error(
                "backend_not_configured",
                &format!("no log-query RPC endpoint is configured for {}", network.id),
            ));
        }
        let params = json!([filter]);
        self.evm_rpc_pool_call(&pool, "eth_getLogs", &params)
    }

    /// Call an EVM JSON-RPC method, failing over across the network's RPC pool: a transport
    /// error, an HTTP error, or a JSON-RPC error on one endpoint advances to the next (PC2's
    /// `baseRpcCall` rotate-on-error). The answer itself is never softened — a definitive
    /// result is returned as-is; only endpoint failures rotate. Returns the LAST error if
    /// every endpoint failed (fail-closed for the caller).
    pub(super) fn evm_rpc(
        &self,
        network: &ChainNetwork,
        method: &str,
        params: Value,
    ) -> Result<Value, Response> {
        let pool = Self::evm_rpc_pool(network);
        if pool.is_empty() {
            return Err(Response::error(
                "backend_not_configured",
                &format!("no RPC endpoint is configured for {}", network.id),
            ));
        }
        self.evm_rpc_pool_call(&pool, method, &params)
    }

    /// Run `method` across a pre-resolved RPC `pool` with rotate-on-error failover AND a bounded
    /// retry-with-backoff on TRANSIENT upstream failures — a rate limit (HTTP 429), a gateway/5xx,
    /// or a transport error — because the public RPC endpoints are out of our control and
    /// intermittently throttle. A full pool pass that fails only transiently is retried after an
    /// exponential backoff; a permanent failure (a real JSON-RPC error, a client 4xx, a missing
    /// result) fails fast without retry. The answer itself is never softened.
    fn evm_rpc_pool_call(
        &self,
        pool: &[&str],
        method: &str,
        params: &Value,
    ) -> Result<Value, Response> {
        const MAX_RPC_ATTEMPTS: usize = 4;
        let mut last_err: Option<Response> = None;
        for attempt in 1..=MAX_RPC_ATTEMPTS {
            let mut any_transient = false;
            for &url in pool {
                match self.evm_rpc_once(url, method, params) {
                    Ok(value) => return Ok(value),
                    Err((transient, response)) => {
                        any_transient = any_transient || transient;
                        last_err = Some(response);
                    }
                }
            }
            if !any_transient || attempt == MAX_RPC_ATTEMPTS {
                break;
            }
            // Exponential backoff between full pool passes: 200ms, 400ms, 800ms.
            std::thread::sleep(std::time::Duration::from_millis(200u64 << (attempt - 1)));
        }
        Err(last_err
            .unwrap_or_else(|| Response::error("upstream_unreachable", "all RPC endpoints failed")))
    }

    /// A single JSON-RPC round trip against one endpoint URL. On failure returns
    /// `(transient, response)`: `transient` is true when the failure is worth retrying against the
    /// pool after a backoff — a transport error, a rate limit (HTTP 429), or a gateway/5xx — and
    /// false when it is a definitive answer we must not retry: a real JSON-RPC error (e.g. a revert),
    /// a client 4xx, or a missing result.
    fn evm_rpc_once(
        &self,
        url: &str,
        method: &str,
        params: &Value,
    ) -> Result<Value, (bool, Response)> {
        let response = self
            .client
            .post(url)
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": method,
                "params": params,
            }))
            .send()
            .map_err(|err| (true, Response::error("upstream_unreachable", &err.to_string())))?;
        let status = response.status();
        if !status.is_success() {
            return Err((
                http_status_is_transient(status.as_u16()),
                Response::error(
                    "upstream_http_error",
                    &format!("upstream returned HTTP {}", status.as_u16()),
                ),
            ));
        }
        let body = response
            .json::<Value>()
            .map_err(|err| (true, Response::error("upstream_invalid_json", &err.to_string())))?;
        if let Some(error) = body.get("error") {
            return Err((false, Response::error("upstream_rpc_error", &error.to_string())));
        }
        body.get("result")
            .cloned()
            .ok_or_else(|| (false, Response::error("upstream_missing_result", "RPC result missing")))
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

/// An upstream HTTP status worth retrying against the RPC pool after a backoff: a rate limit
/// (HTTP 429) or a gateway/server error (5xx). Other 4xx statuses are definitive client-side
/// answers and are NOT retried.
fn http_status_is_transient(status: u16) -> bool {
    status == 429 || (500..=599).contains(&status)
}

#[cfg(test)]
mod backends_retry_tests {
    use super::http_status_is_transient;

    #[test]
    fn only_rate_limit_and_5xx_are_transient() {
        // Retryable: rate limit + the whole 5xx range.
        assert!(http_status_is_transient(429));
        assert!(http_status_is_transient(500));
        assert!(http_status_is_transient(502));
        assert!(http_status_is_transient(503));
        assert!(http_status_is_transient(599));
        // Not retryable: success and definitive client errors.
        assert!(!http_status_is_transient(200));
        assert!(!http_status_is_transient(400));
        assert!(!http_status_is_transient(401));
        assert!(!http_status_is_transient(403));
        assert!(!http_status_is_transient(404));
        assert!(!http_status_is_transient(422));
    }
}
