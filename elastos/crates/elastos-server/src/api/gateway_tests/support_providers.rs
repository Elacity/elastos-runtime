struct MockChainProvider;

const MOCK_PROTECTED_CONTENT_AUTHORITY_GATEWAY: &str = "0x00000000000000000000000000000000000000aa";
const MOCK_PROTECTED_CONTENT_PAY_TOKEN: &str = "0x00000000000000000000000000000000000000bb";
const MOCK_PROTECTED_CONTENT_OPERATIVE: &str = "0x00000000000000000000000000000000000000dd";
const MOCK_PROTECTED_CONTENT_PAYMENT_PROCESSOR: &str = "0x00000000000000000000000000000000000000ff";
const MOCK_PROTECTED_CONTENT_TOKEN_ID: &str = "0x77";
const MOCK_PROTECTED_CONTENT_LISTING_QUANTITY: &str = "0x2";
const MOCK_PROTECTED_CONTENT_LISTING_PRICE: &str = "0x5";
const MOCK_PROTECTED_CONTENT_CHAIN_ID: u64 = 8453;

#[derive(Clone, Copy, PartialEq, Eq)]
enum MockProtectedContentChainMode {
    Success,
    ReceiptError,
    ListingError,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MockProtectedContentPurchaseAccessMode {
    Allow,
    Deny,
    Error,
}

#[derive(Clone)]
struct MockProtectedContentPurchaseFixture {
    native_purchase: bool,
    access_mode: MockProtectedContentPurchaseAccessMode,
    listing_quantity: String,
}

#[derive(Clone)]
struct MockPublishedProtectedContentState {
    files: std::collections::BTreeMap<String, Vec<u8>>,
    receipt: crate::content::SignedAvailabilityReceipt,
}

impl Default for MockProtectedContentPurchaseFixture {
    fn default() -> Self {
        Self {
            native_purchase: false,
            access_mode: MockProtectedContentPurchaseAccessMode::Allow,
            listing_quantity: MOCK_PROTECTED_CONTENT_LISTING_QUANTITY.to_string(),
        }
    }
}

fn mock_content_publish_requests() -> &'static std::sync::Mutex<Vec<serde_json::Value>> {
    static REQUESTS: std::sync::OnceLock<std::sync::Mutex<Vec<serde_json::Value>>> =
        std::sync::OnceLock::new();
    REQUESTS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

fn reset_mock_content_publish_requests() {
    mock_content_publish_requests().lock().unwrap().clear();
}

fn mock_content_publish_request_count() -> usize {
    mock_content_publish_requests().lock().unwrap().len()
}

fn mock_published_protected_content(
) -> &'static std::sync::Mutex<Option<MockPublishedProtectedContentState>> {
    static STATE: std::sync::OnceLock<
        std::sync::Mutex<Option<MockPublishedProtectedContentState>>,
    > = std::sync::OnceLock::new();
    STATE.get_or_init(|| std::sync::Mutex::new(None))
}

fn mock_protected_content_provider_signing_key() -> ed25519_dalek::SigningKey {
    ed25519_dalek::SigningKey::from_bytes(&[0x5a; 32])
}

fn mock_protected_content_provider_signer_did() -> String {
    crate::crypto::domain_separated_sign(
        &mock_protected_content_provider_signing_key(),
        "elastos.content.availability.receipt.v1",
        b"mock-protected-content-provider",
    )
    .1
}

fn mock_protected_content_file(path: &str, bytes: &[u8]) -> crate::content::ContentObjectFile {
    crate::content::ContentObjectFile {
        path: path.to_string(),
        sha256: hex::encode(sha2::Sha256::digest(bytes)),
        size: bytes.len() as u64,
    }
}

fn mock_protected_content_manifest_digest(files: &[crate::content::ContentObjectFile]) -> String {
    let mut hasher = sha2::Sha256::new();
    for file in files {
        hasher.update(file.path.as_bytes());
        hasher.update(b"\0");
        hasher.update(file.sha256.as_bytes());
        hasher.update(b"\0");
        hasher.update(file.size.to_string().as_bytes());
        hasher.update(b"\0");
    }
    format!("sha256:{:x}", hasher.finalize())
}

fn seed_mock_published_protected_content(
    object_identity: &str,
    publisher_did: &str,
    media_identity: &elastos_protected_content_provider_contracts::CencFmp4MediaIdentityV1,
    protected_init: &[u8],
    protected_segments: &[Vec<u8>],
    checked_at: u64,
) {
    let descriptor = media_identity.canonical_bytes().unwrap();
    let mut files = std::collections::BTreeMap::new();
    let mut manifest_files = Vec::with_capacity(protected_segments.len() + 2);

    let identity_path = "protected-content/v1/identity.bin";
    files.insert(identity_path.to_string(), descriptor.clone());
    manifest_files.push(mock_protected_content_file(identity_path, &descriptor));

    let init_path = "protected-content/v1/init.mp4";
    files.insert(init_path.to_string(), protected_init.to_vec());
    manifest_files.push(mock_protected_content_file(init_path, protected_init));

    for (index, segment) in protected_segments.iter().enumerate() {
        let path = format!("protected-content/v1/segments/{index:08}.m4s");
        files.insert(path.clone(), segment.clone());
        manifest_files.push(mock_protected_content_file(&path, segment));
    }
    manifest_files.sort_by(|left, right| left.path.cmp(&right.path));

    let manifest = crate::content::ContentObjectManifest {
        schema: "elastos.content.object.manifest/v1".to_string(),
        kind: "protected-content".to_string(),
        content_digest: mock_protected_content_manifest_digest(&manifest_files),
        files: manifest_files,
        links: Vec::new(),
        object_did: Some(object_identity.to_string()),
        publisher_did: Some(publisher_did.to_string()),
    };
    files.insert(
        crate::content::CONTENT_OBJECT_MANIFEST_PATH.to_string(),
        serde_json::to_vec(&manifest).unwrap(),
    );

    let payload = crate::content::AvailabilityReceipt {
        schema: "elastos.content.availability.receipt/v1".to_string(),
        cid: TEST_CIDV1.to_string(),
        uri: format!("elastos://{TEST_CIDV1}"),
        object_did: Some(object_identity.to_string()),
        publisher_did: publisher_did.to_string(),
        provider: "content".to_string(),
        policy: "replicate:3".to_string(),
        status: "network_available".to_string(),
        replicas: 3,
        peer_selection: json!({}),
        quota: json!({}),
        repair_worker: json!({}),
        storage_market: json!({}),
        repair_graph: json!({}),
        abuse_controls: json!({}),
        accounting: json!({}),
        checked_at,
    };
    let payload_bytes = serde_json::to_string(&serde_json::to_value(&payload).unwrap()).unwrap();
    let (signature, signer_did) = crate::crypto::domain_separated_sign(
        &mock_protected_content_provider_signing_key(),
        "elastos.content.availability.receipt.v1",
        payload_bytes.as_bytes(),
    );
    *mock_published_protected_content().lock().unwrap() =
        Some(MockPublishedProtectedContentState {
            files,
            receipt: crate::content::SignedAvailabilityReceipt {
                payload,
                signature,
                signer_did,
            },
        });
}

fn mock_protected_content_chain_mode() -> &'static std::sync::Mutex<MockProtectedContentChainMode> {
    static MODE: std::sync::OnceLock<std::sync::Mutex<MockProtectedContentChainMode>> =
        std::sync::OnceLock::new();
    MODE.get_or_init(|| std::sync::Mutex::new(MockProtectedContentChainMode::Success))
}

fn reset_mock_protected_content_chain_mode() {
    *mock_protected_content_chain_mode().lock().unwrap() = MockProtectedContentChainMode::Success;
}

fn set_mock_protected_content_chain_receipt_error() {
    *mock_protected_content_chain_mode().lock().unwrap() =
        MockProtectedContentChainMode::ReceiptError;
}

fn set_mock_protected_content_chain_listing_error() {
    *mock_protected_content_chain_mode().lock().unwrap() =
        MockProtectedContentChainMode::ListingError;
}

fn mock_protected_content_purchase_fixture(
) -> &'static std::sync::Mutex<MockProtectedContentPurchaseFixture> {
    static FIXTURE: std::sync::OnceLock<std::sync::Mutex<MockProtectedContentPurchaseFixture>> =
        std::sync::OnceLock::new();
    FIXTURE.get_or_init(|| std::sync::Mutex::new(MockProtectedContentPurchaseFixture::default()))
}

fn reset_mock_protected_content_purchase_fixture() {
    *mock_protected_content_purchase_fixture().lock().unwrap() =
        MockProtectedContentPurchaseFixture::default();
}

fn set_mock_protected_content_purchase_native() {
    mock_protected_content_purchase_fixture()
        .lock()
        .unwrap()
        .native_purchase = true;
}

fn set_mock_protected_content_purchase_access_denied() {
    mock_protected_content_purchase_fixture()
        .lock()
        .unwrap()
        .access_mode = MockProtectedContentPurchaseAccessMode::Deny;
}

fn set_mock_protected_content_purchase_access_error() {
    mock_protected_content_purchase_fixture()
        .lock()
        .unwrap()
        .access_mode = MockProtectedContentPurchaseAccessMode::Error;
}

fn set_mock_protected_content_listing_quantity(quantity: &str) {
    mock_protected_content_purchase_fixture()
        .lock()
        .unwrap()
        .listing_quantity = quantity.to_string();
}

fn mock_chain_raw_requests() -> &'static std::sync::Mutex<Vec<serde_json::Value>> {
    static REQUESTS: std::sync::OnceLock<std::sync::Mutex<Vec<serde_json::Value>>> =
        std::sync::OnceLock::new();
    REQUESTS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

fn reset_mock_chain_raw_requests() {
    mock_chain_raw_requests().lock().unwrap().clear();
}

fn mock_chain_raw_request_count(op: &str) -> usize {
    mock_chain_raw_requests()
        .lock()
        .unwrap()
        .iter()
        .filter(|request| request.get("op").and_then(Value::as_str) == Some(op))
        .count()
}

const MOCK_MANAGED_EVM_ADDRESS: &str = "0x19e7e376e7c213b7e7e7e46cc70a5dd086daff2a";

fn mock_trim_integer_bytes(bytes: &[u8]) -> &[u8] {
    let first = bytes
        .iter()
        .position(|byte| *byte != 0)
        .unwrap_or(bytes.len());
    &bytes[first..]
}

fn mock_managed_evm_signing_key(index: usize) -> Result<EvmSigningKey, ProviderError> {
    let byte = u8::try_from(index)
        .ok()
        .and_then(|value| value.checked_add(0x10))
        .ok_or_else(|| {
            ProviderError::Provider("mock managed EVM key index overflow".to_string())
        })?;
    EvmSigningKey::from_bytes((&[byte; 32]).into())
        .map_err(|err| ProviderError::Provider(err.to_string()))
}

fn mock_managed_evm_address(index: usize) -> Result<String, ProviderError> {
    let key = mock_managed_evm_signing_key(index)?;
    let point = key.verifying_key().to_encoded_point(false);
    let digest = Keccak256::digest(&point.as_bytes()[1..]);
    Ok(format!("0x{}", hex::encode(&digest[12..])))
}

fn mock_managed_evm_key_for_address(address: &str) -> Result<EvmSigningKey, ProviderError> {
    for index in 1..=128 {
        if mock_managed_evm_address(index)?.eq_ignore_ascii_case(address) {
            return mock_managed_evm_signing_key(index);
        }
    }
    Err(ProviderError::Provider(format!(
        "mock has no managed EVM signing key for {address}"
    )))
}

fn mock_sign_eip155_transaction(payload: &Value) -> Result<String, ProviderError> {
    let quantity = |field: &str| {
        exact_payload_quantity(payload, field)
            .map_err(|err| ProviderError::Provider(err.to_string()))
    };
    let chain_id = payload
        .get("chain_id")
        .and_then(Value::as_u64)
        .ok_or_else(|| ProviderError::Provider("mock transaction missing chain_id".to_string()))?;
    let nonce = quantity("nonce")?;
    let gas_price = quantity("gas_price")?;
    let gas_limit = quantity("gas_limit")?;
    let to = exact_payload_bytes(payload, "to")
        .map_err(|err| ProviderError::Provider(err.to_string()))?;
    let value = quantity("value")?;
    let data = exact_payload_bytes(payload, "data")
        .map_err(|err| ProviderError::Provider(err.to_string()))?;
    let chain_id_raw = mock_trim_integer_bytes(&chain_id.to_be_bytes()).to_vec();
    let signing_payload = rlp_encode_list(&[
        rlp_encode_bytes(&nonce),
        rlp_encode_bytes(&gas_price),
        rlp_encode_bytes(&gas_limit),
        rlp_encode_bytes(&to),
        rlp_encode_bytes(&value),
        rlp_encode_bytes(&data),
        rlp_encode_bytes(&chain_id_raw),
        rlp_encode_bytes(&[]),
        rlp_encode_bytes(&[]),
    ]);
    let from = required_test_str(payload, "from")?;
    let signing_key = mock_managed_evm_key_for_address(from)?;
    let signing_hash = Keccak256::digest(signing_payload);
    let (signature, recovery_id) = signing_key
        .sign_prehash_recoverable(&signing_hash)
        .map_err(|err| ProviderError::Provider(err.to_string()))?;
    let signature = signature.to_bytes();
    let v = chain_id
        .checked_mul(2)
        .and_then(|value| value.checked_add(35))
        .and_then(|value| value.checked_add(u64::from(recovery_id.to_byte())))
        .ok_or_else(|| ProviderError::Provider("mock transaction chain id overflow".to_string()))?;
    let v_bytes = v.to_be_bytes();
    let signed = rlp_encode_list(&[
        rlp_encode_bytes(&nonce),
        rlp_encode_bytes(&gas_price),
        rlp_encode_bytes(&gas_limit),
        rlp_encode_bytes(&to),
        rlp_encode_bytes(&value),
        rlp_encode_bytes(&data),
        rlp_encode_bytes(mock_trim_integer_bytes(&v_bytes)),
        rlp_encode_bytes(mock_trim_integer_bytes(&signature[..32])),
        rlp_encode_bytes(mock_trim_integer_bytes(&signature[32..])),
    ]);
    Ok(format!("0x{}", hex::encode(signed)))
}

fn mock_chain_broadcast_counts() -> &'static std::sync::Mutex<HashMap<String, usize>> {
    static COUNTS: std::sync::OnceLock<std::sync::Mutex<HashMap<String, usize>>> =
        std::sync::OnceLock::new();
    COUNTS.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

fn reset_mock_chain_broadcast_count(signed_transaction: &str) {
    let mut counts = mock_chain_broadcast_counts().lock().unwrap();
    counts.remove(signed_transaction);
    drop(counts);
    mock_chain_uncertain_broadcasts()
        .lock()
        .unwrap()
        .remove(signed_transaction);
    if signed_transaction.starts_with("0x")
        && signed_transaction.len().is_multiple_of(2)
        && signed_transaction[2..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        mock_chain_hidden_transaction_hashes()
            .lock()
            .unwrap()
            .remove(&signed_evm_transaction_hash_for_test(signed_transaction));
    }
}

fn mock_chain_broadcast_count(signed_transaction: &str) -> usize {
    let counts = mock_chain_broadcast_counts().lock().unwrap();
    counts.get(signed_transaction).copied().unwrap_or(0)
}

fn mock_chain_uncertain_broadcasts() -> &'static std::sync::Mutex<HashSet<String>> {
    static TRANSACTIONS: std::sync::OnceLock<std::sync::Mutex<HashSet<String>>> =
        std::sync::OnceLock::new();
    TRANSACTIONS.get_or_init(|| std::sync::Mutex::new(HashSet::new()))
}

fn mock_chain_hidden_transaction_hashes() -> &'static std::sync::Mutex<HashSet<String>> {
    static HASHES: std::sync::OnceLock<std::sync::Mutex<HashSet<String>>> =
        std::sync::OnceLock::new();
    HASHES.get_or_init(|| std::sync::Mutex::new(HashSet::new()))
}

fn mark_mock_chain_broadcast_uncertain(signed_transaction: &str, visible: bool) {
    mock_chain_uncertain_broadcasts()
        .lock()
        .unwrap()
        .insert(signed_transaction.to_string());
    let transaction_hash = signed_evm_transaction_hash_for_test(signed_transaction);
    let mut hidden = mock_chain_hidden_transaction_hashes().lock().unwrap();
    if visible {
        hidden.remove(&transaction_hash);
    } else {
        hidden.insert(transaction_hash);
    }
}

fn signed_evm_transaction_hash_for_test(signed_transaction: &str) -> String {
    let raw = hex::decode(signed_transaction.trim_start_matches("0x")).unwrap();
    format!("0x{}", hex::encode(Keccak256::digest(raw)))
}

#[async_trait::async_trait]
impl Provider for MockChainProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "mock chain provider only supports raw requests".into(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["elastos"]
    }

    fn name(&self) -> &'static str {
        "mock-chain-provider"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        mock_chain_raw_requests()
            .lock()
            .unwrap()
            .push(request.clone());
        match request.get("op").and_then(|value| value.as_str()) {
            Some("networks") => Ok(json!({
                "status": "ok",
                "data": {
                    "networks": [
                        {
                            "id": "esc-mainnet",
                            "display_name": "Elastos Smart Chain",
                            "kind": "evm_json_rpc",
                            "chain_id": 20,
                            "native_symbol": "ELA",
                            "provider": "Elastos",
                            "mainnet": true,
                            "explorer_url": "https://esc.elastos.io"
                        }
                    ]
                }
            })),
            Some("status") => Ok(json!({
                "status": "ok",
                "data": {
                    "network": {
                        "id": "esc-mainnet",
                        "display_name": "Elastos Smart Chain",
                        "kind": "evm_json_rpc",
                        "chain_id": 20,
                        "native_symbol": "ELA",
                        "provider": "Elastos",
                        "mainnet": true
                    },
                    "chain_id_hex": "0x14",
                    "block_number_hex": "0x2a",
                    "block_number": 42
                }
            })),
            Some("sync_health") => Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.chain.sync_health/v1",
                    "network": {
                        "id": "esc-mainnet",
                        "display_name": "Elastos Smart Chain",
                        "kind": "evm_json_rpc",
                        "chain_id": 20,
                        "native_symbol": "ELA",
                        "provider": "Elastos",
                        "mainnet": true
                    },
                    "syncing": false,
                    "healthy": true,
                    "latest_block": 42
                }
            })),
            Some("block_number") => Ok(json!({
                "status": "ok",
                "data": {
                    "network": request
                        .get("network")
                        .and_then(|value| value.as_str())
                        .unwrap_or("esc-mainnet"),
                    "block_number_hex": "0x2a",
                    "block_number": 42
                }
            })),
            Some("resolve_protected_content_creator_mint") => Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.chain.protected-content-creator-mint/v1",
                    "network": "base-mainnet",
                    "chain_namespace": "eip155:8453",
                    "function": "mint(string,uint16,bytes,bytes)",
                    "ledger": MOCK_PROTECTED_CONTENT_AUTHORITY_GATEWAY,
                    "pay_token": MOCK_PROTECTED_CONTENT_PAY_TOKEN,
                    "to": MOCK_PROTECTED_CONTENT_AUTHORITY_GATEWAY,
                    "data": format!(
                        "0x{}",
                        hex::encode(Keccak256::digest(
                            serde_json::to_vec(&json!({
                                "creator": required_test_str(request, "creator")?,
                                "token_uri": required_test_str(request, "token_uri")?,
                                "content_access_id": required_test_str(request, "content_access_id")?,
                                "copies": required_test_str(request, "copies")?,
                                "price": required_test_str(request, "price")?,
                            }))
                            .map_err(|err| ProviderError::Provider(err.to_string()))?
                        ))
                    ),
                    "value": "0x0",
                    "content_access_id": required_test_str(request, "content_access_id")?
                        .to_ascii_lowercase(),
                    "signed": false
                }
            })),
            Some("resolve_protected_content_mint_receipt") => {
                if *mock_protected_content_chain_mode().lock().unwrap()
                    == MockProtectedContentChainMode::ReceiptError
                {
                    return Ok(json!({
                        "status": "error",
                        "code": "unavailable",
                        "message": "mock protected-content mint receipt unavailable"
                    }));
                }
                if request.get("op_type_code").and_then(Value::as_u64) != Some(1) {
                    return Ok(json!({
                        "status": "error",
                        "code": "invalid_request",
                        "message": "mock protected-content mint receipt requires BUY_ONCE op type"
                    }));
                }
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "schema": "elastos.chain.protected-content-mint-receipt/v1",
                        "network": required_test_str(request, "network")?,
                        "chain_id": MOCK_PROTECTED_CONTENT_CHAIN_ID,
                        "token_id": MOCK_PROTECTED_CONTENT_TOKEN_ID,
                        "operative": MOCK_PROTECTED_CONTENT_OPERATIVE
                    }
                }))
            }
            Some("resolve_protected_content_verified_listing") => {
                if *mock_protected_content_chain_mode().lock().unwrap()
                    == MockProtectedContentChainMode::ListingError
                {
                    return Ok(json!({
                        "status": "error",
                        "code": "unavailable",
                        "message": "mock protected-content verified listing unavailable"
                    }));
                }
                let fixture = mock_protected_content_purchase_fixture()
                    .lock()
                    .unwrap()
                    .clone();
                let pay_token = if fixture.native_purchase {
                    "0x0000000000000000000000000000000000000000".to_string()
                } else {
                    MOCK_PROTECTED_CONTENT_PAY_TOKEN.to_string()
                };
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "schema": "elastos.chain.protected-content-verified-listing/v1",
                        "network": required_test_str(request, "network")?,
                        "chain_id": MOCK_PROTECTED_CONTENT_CHAIN_ID,
                        "seller": required_test_str(request, "seller")?.to_ascii_lowercase(),
                        "ledger": required_test_str(request, "ledger")?.to_ascii_lowercase(),
                        "token_id": required_test_str(request, "token_id")?.to_ascii_lowercase(),
                        "operative": MOCK_PROTECTED_CONTENT_OPERATIVE,
                        "quantity": fixture.listing_quantity,
                        "price": MOCK_PROTECTED_CONTENT_LISTING_PRICE,
                        "pay_token": pay_token,
                        "payment_processor": (!fixture.native_purchase)
                            .then_some(MOCK_PROTECTED_CONTENT_PAYMENT_PROCESSOR)
                    }
                }))
            }
            Some("resolve_protected_content_purchase") => {
                let fixture = mock_protected_content_purchase_fixture()
                    .lock()
                    .unwrap()
                    .clone();
                let pay_token = if fixture.native_purchase {
                    "0x0000000000000000000000000000000000000000".to_string()
                } else {
                    MOCK_PROTECTED_CONTENT_PAY_TOKEN.to_string()
                };
                let steps = if fixture.native_purchase {
                    json!([{
                        "stage": "buy",
                        "to": MOCK_PROTECTED_CONTENT_AUTHORITY_GATEWAY,
                        "value": MOCK_PROTECTED_CONTENT_LISTING_PRICE,
                        "data": "0x6e61746976655f627579"
                    }])
                } else {
                    json!([
                        {
                            "stage": "approval",
                            "to": MOCK_PROTECTED_CONTENT_PAY_TOKEN,
                            "value": "0x0",
                            "data": "0x617070726f7665"
                        },
                        {
                            "stage": "buy",
                            "to": MOCK_PROTECTED_CONTENT_AUTHORITY_GATEWAY,
                            "value": "0x0",
                            "data": "0x627579"
                        }
                    ])
                };
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "schema": "elastos.chain.protected-content-purchase/v1",
                        "network": required_test_str(request, "network")?,
                        "purchase_quantity": "0x1",
                        "verified_listing": {
                            "chain_id": MOCK_PROTECTED_CONTENT_CHAIN_ID,
                            "seller": required_test_str(request, "seller")?.to_ascii_lowercase(),
                            "ledger": required_test_str(request, "ledger")?.to_ascii_lowercase(),
                            "token_id": required_test_str(request, "token_id")?.to_ascii_lowercase(),
                            "operative": MOCK_PROTECTED_CONTENT_OPERATIVE,
                            "available_quantity": fixture.listing_quantity,
                            "price": MOCK_PROTECTED_CONTENT_LISTING_PRICE,
                            "pay_token": pay_token,
                            "payment_processor": (!fixture.native_purchase)
                                .then_some(MOCK_PROTECTED_CONTENT_PAYMENT_PROCESSOR)
                        },
                        "steps": steps
                    }
                }))
            }
            Some("resolve_protected_content_purchase_access") => {
                let fixture = mock_protected_content_purchase_fixture()
                    .lock()
                    .unwrap()
                    .clone();
                if fixture.access_mode == MockProtectedContentPurchaseAccessMode::Error {
                    return Ok(json!({
                        "status": "error",
                        "code": "stale_protected_content_purchase_access_observation",
                        "message": "mock protected-content purchase access is unavailable"
                    }));
                }
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "schema": "elastos.chain.protected-content-purchase-access/v1",
                        "request_id": required_test_str(request, "request_id")?,
                        "network": required_test_str(request, "network")?,
                        "chain_id": MOCK_PROTECTED_CONTENT_CHAIN_ID,
                        "wallet": required_test_str(request, "wallet")?.to_ascii_lowercase(),
                        "content_access_id": required_test_str(request, "content_access_id")?
                            .to_ascii_lowercase(),
                        "has_access": fixture.access_mode
                            == MockProtectedContentPurchaseAccessMode::Allow,
                        "finalized_block_number": 44,
                        "finalized_block_hash": format!("0x{}", hex::encode([0x44; 32])),
                        "finalized_block_timestamp": crate::auth::now_ts().saturating_sub(5),
                        "observed_at": crate::auth::now_ts(),
                    }
                }))
            }
            Some("node_lifecycle") => Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.chain.node_lifecycle/v1",
                    "network": {
                        "id": "esc-mainnet",
                        "display_name": "Elastos Smart Chain",
                        "kind": "evm_json_rpc",
                        "chain_id": 20,
                        "native_symbol": "ELA",
                        "provider": "Elastos",
                        "mainnet": true
                    },
                    "managed": true,
                    "control_available": true,
                    "control_reason": "operator-approved supervisor configured",
                    "action": request
                        .get("action")
                        .and_then(|value| value.as_str())
                        .unwrap_or("status"),
                    "state": "managed_local",
                    "first_seen_at": 1,
                    "updated_at": 2
                }
            })),
            Some("balance") => Ok(json!({
                "status": "ok",
                "data": {
                    "network": request
                        .get("network")
                        .and_then(|value| value.as_str())
                        .unwrap_or("esc-mainnet"),
                    "address": required_test_str(request, "address")?,
                    "block": request
                        .get("block")
                        .and_then(|value| value.as_str())
                        .unwrap_or("latest"),
                    "balance_hex": "0xde0b6b3a7640000",
                    "native_symbol": "ELA"
                }
            })),
            Some("contract_call") => Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.chain.contract_call/v1",
                    "network": request
                        .get("network")
                        .and_then(|value| value.as_str())
                        .unwrap_or("esc-mainnet"),
                    "to": required_test_str(request, "to")?,
                    "data": required_test_str(request, "data")?,
                    "block": request
                        .get("block")
                        .and_then(|value| value.as_str())
                        .unwrap_or("latest"),
                    "result": "0x0000000000000000000000000000000000000000000000000000000000000042"
                }
            })),
            Some("estimate_gas") => Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.chain.gas_estimate/v1",
                    "network": request
                        .get("network")
                        .and_then(|value| value.as_str())
                        .unwrap_or("esc-mainnet"),
                    "from": required_test_str(request, "from")?,
                    "to": required_test_str(request, "to")?,
                    "value": request.get("value").and_then(|value| value.as_str()).unwrap_or("0x0"),
                    "data": request.get("data").and_then(|value| value.as_str()).unwrap_or("0x"),
                    "gas_limit": "0x5208"
                }
            })),
            Some("transaction_count") => Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.chain.transaction_count/v1",
                    "network": request
                        .get("network")
                        .and_then(|value| value.as_str())
                        .unwrap_or("esc-mainnet"),
                    "address": required_test_str(request, "address")?,
                    "block": request
                        .get("block")
                        .and_then(|value| value.as_str())
                        .unwrap_or("pending"),
                    "nonce": "0x7"
                }
            })),
            Some("gas_price") => Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.chain.gas_price/v1",
                    "network": request
                        .get("network")
                        .and_then(|value| value.as_str())
                        .unwrap_or("esc-mainnet"),
                    "gas_price": "0x3b9aca00"
                }
            })),
            Some("fee_history") => Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.chain.fee_history/v1",
                    "network": request
                        .get("network")
                        .and_then(|value| value.as_str())
                        .unwrap_or("esc-mainnet"),
                    "history": {
                        "oldestBlock": "0x1",
                        "baseFeePerGas": ["0x3b9aca00", "0x3b9aca01"],
                        "gasUsedRatio": [0.5],
                        "reward": [["0x1"]]
                    }
                }
            })),
            Some("code") => Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.chain.code/v1",
                    "network": request
                        .get("network")
                        .and_then(|value| value.as_str())
                        .unwrap_or("esc-mainnet"),
                    "address": required_test_str(request, "address")?,
                    "block": request
                        .get("block")
                        .and_then(|value| value.as_str())
                        .unwrap_or("latest"),
                    "code": "0x60016001"
                }
            })),
            Some("logs") => Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.chain.logs/v1",
                    "network": request
                        .get("network")
                        .and_then(|value| value.as_str())
                        .unwrap_or("esc-mainnet"),
                    "logs": [{
                        "address": "0x2222222222222222222222222222222222222222",
                        "blockNumber": "0x2a",
                        "data": "0x",
                        "topics": []
                    }]
                }
            })),
            Some("transaction") => {
                let hash = required_test_str(request, "hash")?;
                if hash == "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
                    || mock_chain_hidden_transaction_hashes()
                        .lock()
                        .unwrap()
                        .contains(hash)
                {
                    return Ok(json!({
                        "status": "ok",
                        "data": {
                            "network": required_test_str(request, "network")?,
                            "hash": hash,
                            "transaction": null
                        }
                    }));
                }
                let observed_hash = if hash
                    == "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
                {
                    "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                } else {
                    hash
                };
                let observed_from = match hash {
                    "0xdddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
                    | "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff" => {
                        "0x3333333333333333333333333333333333333333"
                    }
                    "0xcccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc" => {
                        "0x4444444444444444444444444444444444444444"
                    }
                    _ => "0x1111111111111111111111111111111111111111",
                };
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "network": request
                            .get("network")
                            .and_then(|value| value.as_str())
                            .unwrap_or("esc-mainnet"),
                        "hash": hash,
                        "transaction": {
                            "hash": observed_hash,
                            "from": observed_from,
                            "to": "0x2222222222222222222222222222222222222222",
                            "value": "0x1",
                            "blockNumber": "0x2a"
                        }
                    }
                }))
            }
            Some("receipt") => {
                let hash = required_test_str(request, "hash")?;
                if hash == "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
                    || mock_chain_hidden_transaction_hashes()
                        .lock()
                        .unwrap()
                        .contains(hash)
                {
                    return Ok(json!({
                        "status": "ok",
                        "data": {
                            "network": required_test_str(request, "network")?,
                            "hash": hash,
                            "receipt": null
                        }
                    }));
                }
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "network": request
                            .get("network")
                            .and_then(|value| value.as_str())
                            .unwrap_or("esc-mainnet"),
                        "hash": hash,
                        "receipt": {
                            "transactionHash": hash,
                            "status": "0x1",
                            "blockNumber": "0x2a",
                            "logs": []
                        }
                    }
                }))
            }
            Some("prepare_transaction") => {
                let network = required_test_str(request, "network")?;
                let chain_id = if network == "base-mainnet" { 8453 } else { 20 };
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "schema": "elastos.chain.unsigned_transaction_intent/v1",
                        "transaction_type": "eip155_legacy",
                        "network": {
                            "id": network,
                            "display_name": if network == "base-mainnet" { "Base" } else { "Elastos Smart Chain" },
                            "kind": "evm_json_rpc",
                            "chain_id": chain_id,
                            "native_symbol": if network == "base-mainnet" { "ETH" } else { "ELA" },
                            "provider": if network == "base-mainnet" { "Base" } else { "Elastos" },
                            "mainnet": true
                        },
                        "from": required_test_str(request, "from")?,
                        "to": required_test_str(request, "to")?,
                        "value": request.get("value").and_then(|value| value.as_str()).unwrap_or("0x0"),
                        "data": request.get("data").and_then(|value| value.as_str()).unwrap_or("0x"),
                        "chain_id": chain_id,
                        "nonce": "0x1",
                        "gas_price": "0x3b9aca00",
                        "gas_limit": "0x5208",
                        "requires_wallet_approval": true,
                        "wallet_intent": "transaction_intent"
                    }
                }))
            }
            Some("broadcast_transaction") => {
                let signed_transaction = required_test_str(request, "signed_transaction")?;
                let mut counts = mock_chain_broadcast_counts().lock().unwrap();
                *counts.entry(signed_transaction.to_string()).or_insert(0) += 1;
                drop(counts);
                if mock_chain_uncertain_broadcasts()
                    .lock()
                    .unwrap()
                    .contains(signed_transaction)
                {
                    return Err(ProviderError::Provider(
                        "simulated uncertain Chain broadcast transport".to_string(),
                    ));
                }
                let transaction_hash = signed_evm_transaction_hash_for_test(signed_transaction);
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "schema": "elastos.chain.broadcast_receipt/v1",
                        "network": required_test_str(request, "network")?,
                        "transaction_hash": transaction_hash
                    }
                }))
            }
            Some("erc1271_is_valid_signature") => {
                let signature = required_test_str(request, "signature")?;
                let signature_bytes = hex::decode(signature.trim_start_matches("0x"))
                    .map_err(|err| ProviderError::Provider(err.to_string()))?;
                let signature_hash =
                    format!("0x{}", hex::encode(sha2::Sha256::digest(signature_bytes)));
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "schema": "elastos.chain.erc1271_proof/v1",
                        "network": {
                            "id": "esc-mainnet",
                            "display_name": "Elastos Smart Chain",
                            "kind": "evm_json_rpc",
                            "chain_id": 20,
                            "native_symbol": "ELA",
                            "provider": "Elastos",
                            "mainnet": true
                        },
                        "chain_id": 20,
                        "contract": required_test_str(request, "contract")?,
                        "message_hash": required_test_str(request, "message_hash")?,
                        "signature_hash": signature_hash,
                        "valid": true,
                        "magic_value": "0x1626ba7e",
                        "checked_at": crate::auth::now_ts()
                    }
                }))
            }
            _ => Ok(json!({
                "status": "error",
                "code": "unsupported",
                "message": "unsupported mock chain op"
            })),
        }
    }
}

struct MockContentProvider;

#[async_trait::async_trait]
impl Provider for MockContentProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "mock content provider only supports raw requests".into(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["content"]
    }

    fn name(&self) -> &'static str {
        "mock-content-provider"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        if request.get("cid").and_then(|value| value.as_str()) == Some(TEST_CIDV1) {
            let published = mock_published_protected_content().lock().unwrap().clone();
            if let Some(published) = published {
                match (
                    request.get("op").and_then(|value| value.as_str()),
                    request.get("path").and_then(|value| value.as_str()),
                ) {
                    (Some("status"), _) => {
                        return Ok(json!({
                            "status": "ok",
                            "data": {
                                "cid": TEST_CIDV1,
                                "uri": format!("elastos://{TEST_CIDV1}"),
                                "availability": {
                                    "status": "network_available",
                                    "provider": "mock-content-provider",
                                    "replicas": 3
                                },
                                "receipt": published.receipt,
                            }
                        }));
                    }
                    (Some("fetch"), Some(path)) => {
                        if let Some(bytes) = published.files.get(path) {
                            return Ok(json!({
                                "status": "ok",
                                "data": {
                                    "cid": TEST_CIDV1,
                                    "path": path,
                                    "data": base64::engine::general_purpose::STANDARD.encode(bytes),
                                    "availability": {
                                        "status": "network_available",
                                        "provider": "mock-content-provider",
                                        "replicas": 3
                                    }
                                }
                            }));
                        }
                    }
                    _ => {}
                }
            }
        }
        match (
            request.get("op").and_then(|value| value.as_str()),
            request.get("cid").and_then(|value| value.as_str()),
            request.get("path").and_then(|value| value.as_str()),
        ) {
            (Some("fetch"), Some(TEST_CIDV1), Some("index.html")) => Ok(json!({
                "status": "ok",
                "data": {
                    "cid": TEST_CIDV1,
                    "path": "index.html",
                    "data": base64::engine::general_purpose::STANDARD.encode(b"<html>content provider</html>"),
                    "availability": {
                        "status": "local_pinned",
                        "provider": "mock-content-provider",
                        "replicas": 1
                    }
                }
            })),
            (Some("fetch"), Some(TEST_CIDV1), None) => Ok(json!({
                "status": "ok",
                "data": {
                    "cid": TEST_CIDV1,
                    "path": "",
                    "data": base64::engine::general_purpose::STANDARD.encode(b"raw-content-provider-bytes"),
                    "availability": {
                        "status": "local_pinned",
                        "provider": "mock-content-provider",
                        "replicas": 1
                    }
                }
            })),
            (Some("publish"), _, _) => {
                if request.get("object_kind").and_then(|value| value.as_str()) != Some("sealed") {
                    mock_content_publish_requests()
                        .lock()
                        .unwrap()
                        .push(request.clone());
                }
                if request.get("object_kind").and_then(|value| value.as_str()) == Some("sealed") {
                    validate_mock_sealed_publish_request(request)?;
                }
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "cid": TEST_CIDV1,
                        "uri": format!("elastos://{}", TEST_CIDV1),
                        "availability": {
                            "status": "local_pinned",
                            "provider": "mock-content-provider",
                            "replicas": 1
                        },
                        "receipt": {
                            "schema": "elastos.content.availability.receipt/v1",
                            "cid": TEST_CIDV1
                        }
                    }
                }))
            }
            (Some("unpublish"), _, _) => Ok(json!({
                "status": "ok",
                "data": {
                    "cid": TEST_CIDV1,
                    "uri": format!("elastos://{}", TEST_CIDV1),
                    "availability": {
                        "status": "local_unpinned",
                        "provider": "mock-content-provider",
                        "replicas": 0
                    },
                    "receipt": {
                        "schema": "elastos.content.availability.receipt/v1",
                        "cid": TEST_CIDV1,
                        "status": "local_unpinned"
                    }
                }
            })),
            (Some("repair"), _, _) => Ok(json!({
                "status": "ok",
                "data": {
                    "cid": TEST_CIDV1,
                    "uri": format!("elastos://{}", TEST_CIDV1),
                    "availability": {
                        "status": "local_pinned",
                        "provider": "mock-content-provider",
                        "replicas": 1
                    },
                    "receipt": {
                        "schema": "elastos.content.availability.receipt/v1",
                        "cid": TEST_CIDV1,
                        "status": "local_pinned"
                    }
                }
            })),
            (Some("status"), _, _) => Ok(json!({
                "status": "ok",
                "data": {
                    "cid": TEST_CIDV1,
                    "uri": format!("elastos://{}", TEST_CIDV1),
                    "availability": {
                        "status": "local_pinned",
                        "provider": "mock-content-provider",
                        "replicas": 1
                    }
                }
            })),
            _ => Ok(json!({
                "status": "error",
                "code": "not_found",
                "message": "mock content not found"
            })),
        }
    }
}

fn validate_mock_sealed_publish_request(request: &serde_json::Value) -> Result<(), ProviderError> {
    let files = request
        .get("files")
        .and_then(|value| value.as_array())
        .ok_or_else(|| ProviderError::Provider("sealed publish files are required".into()))?;
    let sealed_entry = files
        .iter()
        .find(|entry| entry.get("path").and_then(|value| value.as_str()) == Some("sealed.json"))
        .ok_or_else(|| ProviderError::Provider("sealed publish requires sealed.json".into()))?;
    let sealed_data = sealed_entry
        .get("data")
        .and_then(|value| value.as_str())
        .ok_or_else(|| ProviderError::Provider("sealed.json data is required".into()))?;
    let sealed_bytes = base64::engine::general_purpose::STANDARD
        .decode(sealed_data)
        .map_err(|err| ProviderError::Provider(err.to_string()))?;
    let sealed_object: elastos_common::protected_content::SealedObjectV1 =
        serde_json::from_slice(&sealed_bytes)
            .map_err(|err| ProviderError::Provider(err.to_string()))?;
    let links = request
        .get("links")
        .and_then(|value| value.as_array())
        .ok_or_else(|| ProviderError::Provider("sealed publish links are required".into()))?;
    for (rel, cid) in [
        (
            "availability.receipt",
            sealed_object.availability_receipt_cid.as_str(),
        ),
        ("payload", sealed_object.payload_cid.as_str()),
        ("rights.policy", sealed_object.rights_policy_cid.as_str()),
    ] {
        if !links.iter().any(|link| {
            link.get("rel").and_then(|value| value.as_str()) == Some(rel)
                && link.get("cid").and_then(|value| value.as_str()) == Some(cid)
        }) {
            return Err(ProviderError::Provider(format!(
                "sealed publish missing {rel} link"
            )));
        }
    }
    if !links
        .iter()
        .any(|link| link.get("rel").and_then(|value| value.as_str()) == Some("provenance"))
    {
        return Err(ProviderError::Provider(
            "sealed publish missing provenance link".into(),
        ));
    }
    if serde_json::to_string(&sealed_object)
        .map_err(|err| ProviderError::Provider(err.to_string()))?
        .contains("raw_cek")
    {
        return Err(ProviderError::Provider(
            "sealed publish must not expose raw CEK".into(),
        ));
    }
    Ok(())
}

struct MockDrmProvider;
struct MockRightsProvider;
struct MockKeyProvider;
struct MockDecryptProvider;

#[async_trait::async_trait]
impl Provider for MockDrmProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "mock drm provider only supports raw requests".into(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["drm"]
    }

    fn name(&self) -> &'static str {
        "mock-drm-provider"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        match request.get("op").and_then(|value| value.as_str()) {
            Some("status") => Ok(json!({
                "status": "ok",
                "data": {
                    "provider": "drm",
                    "configured": true,
                    "supported_operations": ["status", "open"],
                    "blocked_authority": ["raw_cek", "chain_rpc", "wallet_rpc"],
                    "contract": {
                        "schema": "elastos.protected-content.drm-provider/v1",
                        "fixture": true
                    }
                }
            })),
            Some("open") => {
                let request = request
                    .get("request")
                    .ok_or_else(|| ProviderError::Provider("drm request is required".into()))?;
                let object = request
                    .get("object")
                    .ok_or_else(|| ProviderError::Provider("sealed object is required".into()))?;
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "schema": "elastos.drm.open.receipt/v1",
                        "provider": "drm-provider",
                        "status": "accepted",
                        "payload_cid": object
                            .get("payload_cid")
                            .and_then(|value| value.as_str())
                            .unwrap_or(TEST_CIDV1),
                        "principal_id": required_test_str(request, "principal_id")?,
                        "session_id": required_test_str(request, "session_id")?,
                        "action": required_test_str(request, "action")?,
                        "fixture": true
                    }
                }))
            }
            _ => Ok(json!({
                "status": "error",
                "code": "unsupported",
                "message": "unsupported mock drm op"
            })),
        }
    }
}

#[async_trait::async_trait]
impl Provider for MockRightsProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "mock rights provider only supports raw requests".into(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["rights"]
    }

    fn name(&self) -> &'static str {
        "mock-rights-provider"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        match request.get("op").and_then(|value| value.as_str()) {
            Some("status") => Ok(json!({
                "status": "ok",
                "data": {
                    "provider": "rights",
                    "configured": true,
                    "supported_operations": ["status", "has_access_by_content_id"],
                    "blocked_authority": ["chain_rpc", "wallet_rpc", "raw_cek"],
                    "contract": {
                        "schema": "elastos.protected-content.rights-provider/v1",
                        "fixture": true
                    }
                }
            })),
            Some("has_access_by_content_id") => {
                let request = request
                    .get("request")
                    .ok_or_else(|| ProviderError::Provider("rights request is required".into()))?;
                let content_id = required_test_str(request, "content_id")?;
                let principal_id = required_test_str(request, "principal_id")?;
                let session_id = required_test_str(request, "session_id")?;
                let right = required_test_str(request, "right")?;
                let allowed = right == "view" && !principal_id.contains("blocked");
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "schema": "elastos.rights.decision.receipt/v1",
                        "request_id": "rights:fixture",
                        "content_id": content_id,
                        "principal_id": principal_id,
                        "session_id": session_id,
                        "right": right,
                        "provider": "rights-provider",
                        "allowed": allowed,
                        "issued_at": 1_800_000_000u64,
                        "expires_at": 1_900_000_000u64
                    }
                }))
            }
            _ => Ok(json!({
                "status": "error",
                "code": "unsupported",
                "message": "unsupported mock rights op"
            })),
        }
    }
}

#[async_trait::async_trait]
impl Provider for MockKeyProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "mock key provider only supports raw requests".into(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["key"]
    }

    fn name(&self) -> &'static str {
        "mock-key-provider"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        match request.get("op").and_then(|value| value.as_str()) {
            Some("status") => Ok(json!({
                "status": "ok",
                "data": {
                    "provider": "key",
                    "configured": true,
                    "supported_operations": ["status", "release"],
                    "blocked_authority": ["raw_cek", "kms_node_credentials"],
                    "contract": {
                        "schema": "elastos.protected-content.key-provider/v1",
                        "fixture": true
                    }
                }
            })),
            Some("release") => {
                let request = request
                    .get("request")
                    .ok_or_else(|| ProviderError::Provider("key request is required".into()))?;
                if request
                    .get("rights_receipt")
                    .and_then(|receipt| receipt.get("allowed"))
                    .and_then(|value| value.as_bool())
                    != Some(true)
                {
                    return Ok(json!({
                        "status": "error",
                        "code": "denied",
                        "message": "rights receipt denied key release"
                    }));
                }
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "schema": "elastos.release.receipt/v1",
                        "request_id": required_test_str(request, "request_id")?,
                        "object_cid": required_test_str(request, "object_cid")?,
                        "principal_id": required_test_str(request, "principal_id")?,
                        "session_id": required_test_str(request, "session_id")?,
                        "action": required_test_str(request, "action")?,
                        "provider": "key-provider",
                        "status": "released",
                        "issued_at": 1_800_000_000u64,
                        "expires_at": request
                            .get("expires_at")
                            .and_then(|value| value.as_u64())
                            .unwrap_or(1_900_000_000u64)
                    }
                }))
            }
            _ => Ok(json!({
                "status": "error",
                "code": "unsupported",
                "message": "unsupported mock key op"
            })),
        }
    }
}

#[async_trait::async_trait]
impl Provider for MockDecryptProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "mock decrypt provider only supports raw requests".into(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["decrypt"]
    }

    fn name(&self) -> &'static str {
        "mock-decrypt-provider"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        match request.get("op").and_then(|value| value.as_str()) {
            Some("status") => Ok(json!({
                "status": "ok",
                "data": {
                    "provider": "decrypt",
                    "configured": true,
                    "supported_operations": ["status", "open_session"],
                    "blocked_authority": ["raw_cek", "raw_plaintext", "filesystem"],
                    "contract": {
                        "schema": "elastos.protected-content.decrypt-provider/v1",
                        "fixture": true
                    }
                }
            })),
            Some("open_session") => {
                let request = request
                    .get("request")
                    .ok_or_else(|| ProviderError::Provider("decrypt request is required".into()))?;
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "schema": "elastos.decrypt.session/v1",
                        "session_id": "decrypt-session:fixture",
                        "object_cid": required_test_str(request, "object_cid")?,
                        "viewer_interface": required_test_str(request, "viewer_interface")?,
                        "output": "viewer_capsule_session:fixture",
                        "expires_at": request
                            .get("expires_at")
                            .and_then(|value| value.as_u64())
                            .unwrap_or(1_900_000_000u64)
                    }
                }))
            }
            _ => Ok(json!({
                "status": "error",
                "code": "unsupported",
                "message": "unsupported mock decrypt op"
            })),
        }
    }
}

struct MockExternalObjectProvider {
    data_dir: std::path::PathBuf,
}

#[async_trait::async_trait]
impl Provider for MockExternalObjectProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "mock external object provider only supports raw requests".into(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["object"]
    }

    fn name(&self) -> &'static str {
        "mock-external-object-provider"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        Ok(crate::library::handle_object_provider_raw_request(
            &self.data_dir,
            request,
        ))
    }
}

#[derive(Clone)]
struct MockCachedWebSpaceObject {
    bytes: Vec<u8>,
    sync_state: &'static str,
}

#[derive(Default)]
struct MockWebSpaceProvider {
    cached: std::sync::Mutex<BTreeMap<String, MockCachedWebSpaceObject>>,
}

struct MockWebSpaceAdapterProvider;
struct MockOperatorWebSpaceAdapterProvider;

fn mock_operator_archive_zip_bytes() -> Vec<u8> {
    use std::io::Write as _;

    let cursor = std::io::Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(cursor);
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    writer.start_file("alpha.txt", options).unwrap();
    writer.write_all(b"zip alpha").unwrap();
    writer.add_directory("Nested/", options).unwrap();
    writer.start_file("Nested/deep.txt", options).unwrap();
    writer.write_all(b"zip nested").unwrap();
    writer.finish().unwrap().into_inner()
}

impl MockWebSpaceProvider {
    fn cached_object(&self, path: &str) -> Option<MockCachedWebSpaceObject> {
        self.cached.lock().ok()?.get(path).cloned()
    }

    fn store_cached_object(
        &self,
        path: &str,
        request: &serde_json::Value,
    ) -> Result<MockCachedWebSpaceObject, ProviderError> {
        let bytes = request
            .get("content")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| ProviderError::Provider("mock cache missing content".into()))?
            .iter()
            .map(|value| {
                value
                    .as_u64()
                    .filter(|byte| *byte <= u8::MAX as u64)
                    .map(|byte| byte as u8)
                    .ok_or_else(|| ProviderError::Provider("mock cache byte out of range".into()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let cached = MockCachedWebSpaceObject {
            bytes,
            sync_state: "manual_idle",
        };
        self.cached
            .lock()
            .map_err(|_| ProviderError::Provider("mock cache lock poisoned".into()))?
            .insert(path.to_string(), cached.clone());
        Ok(cached)
    }

    fn store_written_object(
        &self,
        path: &str,
        request: &serde_json::Value,
    ) -> Result<MockCachedWebSpaceObject, ProviderError> {
        let bytes = request
            .get("content")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| ProviderError::Provider("mock write missing content".into()))?
            .iter()
            .map(|value| {
                value
                    .as_u64()
                    .filter(|byte| *byte <= u8::MAX as u64)
                    .map(|byte| byte as u8)
                    .ok_or_else(|| ProviderError::Provider("mock write byte out of range".into()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let cached = MockCachedWebSpaceObject {
            bytes,
            sync_state: "manual_pending",
        };
        self.cached
            .lock()
            .map_err(|_| ProviderError::Provider("mock cache lock poisoned".into()))?
            .insert(path.to_string(), cached.clone());
        Ok(cached)
    }

    fn mark_synced(&self, path: &str) -> Result<Option<MockCachedWebSpaceObject>, ProviderError> {
        let mut cached = self
            .cached
            .lock()
            .map_err(|_| ProviderError::Provider("mock cache lock poisoned".into()))?;
        let Some(object) = cached.get_mut(path) else {
            return Ok(None);
        };
        object.sync_state = "manual_synced";
        Ok(Some(object.clone()))
    }
}

#[async_trait::async_trait]
impl Provider for MockWebSpaceProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "mock WebSpace provider only supports raw requests".into(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["webspace"]
    }

    fn name(&self) -> &'static str {
        "mock-webspace-provider"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        let path = request
            .get("path")
            .and_then(|value| value.as_str())
            .unwrap_or("localhost://WebSpaces");
        match request.get("op").and_then(|value| value.as_str()) {
            Some("list") if path == "localhost://WebSpaces" => Ok(json!({
                "status": "ok",
                "data": [
                    {
                        "name": "Elastos",
                        "is_file": false,
                        "is_dir": true,
                        "size": 0,
                        "provider": "mock-webspace-provider",
                        "resolver_state": "resolved",
                        "resolver": "builtin",
                        "cache_policy": "metadata-only",
                        "sync_policy": "manual",
                        "object_id": "object:webspace:elastos",
                        "head_id": "head:webspace:elastos",
                        "cache_state": "metadata_cached",
                        "sync_state": "manual_idle",
                        "kind": "dynamic-webspace",
                        "readonly": true
                    },
                    {
                        "name": "Cloud",
                        "is_file": false,
                        "is_dir": true,
                        "size": 0,
                        "target_uri": "cloud://drive",
                        "provider": "mock-webspace-provider",
                        "resolver_state": "mounted-readonly",
                        "resolver": "cloud-drive",
                        "cache_policy": "metadata-and-thumbnails",
                        "sync_policy": "manual",
                        "object_id": "object:webspace:cloud",
                        "head_id": "head:webspace:cloud",
                        "cache_state": "metadata_cached",
                        "sync_state": "manual_idle",
                        "kind": "mounted-webspace",
                        "readonly": true
                    },
                    {
                        "name": "Operator",
                        "is_file": false,
                        "is_dir": true,
                        "size": 0,
                        "target_uri": "operator://drive",
                        "provider": "mock-webspace-provider",
                        "resolver_state": "mounted-readonly",
                        "resolver": "operator-drive",
                        "cache_policy": "metadata-and-bytes",
                        "sync_policy": "manual",
                        "object_id": "object:webspace:operator",
                        "head_id": "head:webspace:operator",
                        "cache_state": "metadata_cached",
                        "sync_state": "manual_idle",
                        "kind": "mounted-webspace",
                        "readonly": true
                    },
                    {
                        "name": "OperatorMutable",
                        "is_file": false,
                        "is_dir": true,
                        "size": 0,
                        "target_uri": "operator://drive/Writable",
                        "provider": "mock-webspace-provider",
                        "resolver_state": "mounted-mutable",
                        "resolver": "operator-drive",
                        "cache_policy": "metadata-and-bytes",
                        "sync_policy": "manual",
                        "object_id": "object:webspace:operator-mutable",
                        "head_id": "head:webspace:operator-mutable",
                        "cache_state": "content_cached",
                        "sync_state": "manual_idle",
                        "kind": "mounted-webspace",
                        "readonly": false,
                        "access_policy": "owner-writable"
                    },
                    {
                        "name": "Mutable",
                        "is_file": false,
                        "is_dir": true,
                        "size": 0,
                        "target_uri": "local://mutable",
                        "provider": "mock-webspace-provider",
                        "resolver_state": "mounted-mutable",
                        "resolver": "local-materialized",
                        "cache_policy": "metadata-and-bytes",
                        "sync_policy": "manual",
                        "object_id": "object:webspace:mutable",
                        "head_id": "head:webspace:mutable",
                        "cache_state": "content_cached",
                        "sync_state": "manual_idle",
                        "kind": "mounted-webspace",
                        "readonly": false,
                        "access_policy": "owner-writable"
                    }
                ]
            })),
            Some("list") if path == "localhost://WebSpaces/Elastos" => Ok(json!({
                "status": "ok",
                "data": [
                    { "name": "_meta.json", "is_file": true, "is_dir": false, "size": 96, "resolver": "builtin", "cache_policy": "metadata-only", "sync_policy": "manual", "object_id": "object:webspace:elastos-meta", "head_id": "head:webspace:elastos-meta", "cache_state": "metadata_cached", "sync_state": "manual_idle", "kind": "metadata" },
                    { "name": "content", "is_file": false, "is_dir": true, "size": 0, "target_uri": "elastos://<cid>", "resolver": "builtin", "cache_policy": "metadata-only", "sync_policy": "manual", "object_id": "object:webspace:content", "head_id": "head:webspace:content", "cache_state": "metadata_cached", "sync_state": "manual_idle", "kind": "folder-handle" },
                    { "name": "peer", "is_file": false, "is_dir": true, "size": 0, "target_uri": "elastos://peer/", "resolver": "builtin", "cache_policy": "metadata-only", "sync_policy": "manual", "object_id": "object:webspace:peer", "head_id": "head:webspace:peer", "cache_state": "metadata_cached", "sync_state": "manual_idle", "kind": "folder-handle" },
                    { "name": "did", "is_file": false, "is_dir": true, "size": 0, "target_uri": "elastos://did/", "resolver": "builtin", "cache_policy": "metadata-only", "sync_policy": "manual", "object_id": "object:webspace:did", "head_id": "head:webspace:did", "cache_state": "metadata_cached", "sync_state": "manual_idle", "kind": "folder-handle" },
                    { "name": "ai", "is_file": false, "is_dir": true, "size": 0, "target_uri": "elastos://ai/", "resolver": "builtin", "cache_policy": "metadata-only", "sync_policy": "manual", "object_id": "object:webspace:ai", "head_id": "head:webspace:ai", "cache_state": "metadata_cached", "sync_state": "manual_idle", "kind": "folder-handle" }
                ]
            })),
            Some("list") if path == "localhost://WebSpaces/Cloud" => Ok(json!({
                "status": "ok",
                "data": [
                    { "name": "_meta.json", "is_file": true, "is_dir": false, "size": 96, "resolver": "cloud-drive", "cache_policy": "metadata-and-thumbnails", "sync_policy": "manual", "object_id": "object:webspace:cloud-meta", "head_id": "head:webspace:cloud-meta", "cache_state": "metadata_cached", "sync_state": "manual_idle", "kind": "metadata" },
                    { "name": "Drive", "is_file": false, "is_dir": true, "size": 0, "target_uri": "cloud://drive/Drive", "resolver": "cloud-drive", "resolver_state": "indexed", "cache_policy": "metadata-and-thumbnails", "sync_policy": "manual", "object_id": "object:webspace:cloud-drive", "head_id": "head:webspace:cloud-drive", "cache_state": "metadata_cached", "sync_state": "manual_idle", "kind": "indexed-directory", "readonly": true },
                    { "name": "Shared", "is_file": false, "is_dir": true, "size": 0, "target_uri": "cloud://drive/shared", "resolver": "cloud-drive", "resolver_state": "indexed-virtual", "cache_policy": "metadata-and-thumbnails", "sync_policy": "manual", "object_id": "object:webspace:cloud-shared", "head_id": "head:webspace:cloud-shared", "cache_state": "metadata_cached", "sync_state": "manual_idle", "kind": "indexed-directory", "readonly": true }
                ]
            })),
            Some("list") if path == "localhost://WebSpaces/Cloud/Drive" => Ok(json!({
                "status": "ok",
                "data": [
                    { "name": "_meta.json", "is_file": true, "is_dir": false, "size": 96, "resolver": "cloud-drive", "cache_policy": "metadata-and-thumbnails", "sync_policy": "manual", "object_id": "object:webspace:cloud-drive-meta", "head_id": "head:webspace:cloud-drive-meta", "cache_state": "metadata_cached", "sync_state": "manual_idle", "kind": "metadata" },
                    { "name": "Project X", "is_file": false, "is_dir": true, "size": 0, "target_uri": "cloud://drive/Drive/Project X", "resolver": "cloud-drive", "resolver_state": "indexed-virtual", "cache_policy": "metadata-and-thumbnails", "sync_policy": "manual", "object_id": "object:webspace:cloud-project", "head_id": "head:webspace:cloud-project", "cache_state": "metadata_cached", "sync_state": "manual_idle", "kind": "indexed-directory", "readonly": true }
                ]
            })),
            Some("list") if path == "localhost://WebSpaces/Cloud/Drive/Project X" => Ok(json!({
                "status": "ok",
                "data": [
                    { "name": "_meta.json", "is_file": true, "is_dir": false, "size": 96, "resolver": "cloud-drive", "cache_policy": "metadata-and-thumbnails", "sync_policy": "manual", "object_id": "object:webspace:cloud-project-meta", "head_id": "head:webspace:cloud-project-meta", "cache_state": "metadata_cached", "sync_state": "manual_idle", "kind": "metadata" },
                    { "name": "file.pdf", "is_file": true, "is_dir": false, "size": 256, "target_uri": "cloud://drive/Drive/Project X/file.pdf", "resolver": "cloud-drive", "resolver_state": "indexed", "cache_policy": "metadata-and-thumbnails", "sync_policy": "manual", "object_id": "object:webspace:cloud-project-file", "head_id": "head:webspace:cloud-project-file", "cache_state": "metadata_cached", "sync_state": "manual_idle", "kind": "indexed-file", "readonly": true }
                ]
            })),
            Some("list") if path == "localhost://WebSpaces/Operator" => Ok(json!({
                "status": "ok",
                "data": [
                    { "name": "_meta.json", "is_file": true, "is_dir": false, "size": 96, "resolver": "operator-drive", "cache_policy": "metadata-and-bytes", "sync_policy": "manual", "object_id": "object:webspace:operator-meta", "head_id": "head:webspace:operator-meta", "cache_state": "metadata_cached", "sync_state": "manual_idle", "kind": "metadata" },
                    { "name": "Projects", "is_file": false, "is_dir": true, "size": 0, "target_uri": "operator://drive/Projects", "resolver": "operator-drive", "resolver_state": "indexed", "cache_policy": "metadata-and-bytes", "sync_policy": "manual", "object_id": "object:webspace:operator-projects", "head_id": "head:webspace:operator-projects", "cache_state": "metadata_cached", "sync_state": "manual_idle", "kind": "indexed-directory", "readonly": true }
                ]
            })),
            Some("list") if path == "localhost://WebSpaces/Operator/Projects" => Ok(json!({
                "status": "ok",
                "data": [
                    { "name": "_meta.json", "is_file": true, "is_dir": false, "size": 96, "resolver": "operator-drive", "cache_policy": "metadata-and-bytes", "sync_policy": "manual", "object_id": "object:webspace:operator-projects-meta", "head_id": "head:webspace:operator-projects-meta", "cache_state": "metadata_cached", "sync_state": "manual_idle", "kind": "metadata" },
                    { "name": "Brief.md", "is_file": true, "is_dir": false, "size": 512, "target_uri": "operator://drive/Projects/Brief.md", "resolver": "operator-drive", "resolver_state": "indexed", "cache_policy": "metadata-and-bytes", "sync_policy": "manual", "object_id": "object:webspace:operator-brief", "head_id": "head:webspace:operator-brief", "cache_state": "metadata_cached", "sync_state": "manual_idle", "kind": "indexed-file", "readonly": true },
                    { "name": "Bundle.zip", "is_file": true, "is_dir": false, "size": mock_operator_archive_zip_bytes().len(), "target_uri": "operator://drive/Projects/Bundle.zip", "resolver": "operator-drive", "resolver_state": "indexed", "cache_policy": "metadata-and-bytes", "sync_policy": "manual", "object_id": "object:webspace:operator-bundle", "head_id": "head:webspace:operator-bundle", "cache_state": "metadata_cached", "sync_state": "manual_idle", "kind": "indexed-file", "readonly": true }
                ]
            })),
            Some("list") if path == "localhost://WebSpaces/OperatorMutable" => Ok(json!({
                "status": "ok",
                "data": [
                    { "name": "_meta.json", "is_file": true, "is_dir": false, "size": 96, "resolver": "operator-drive", "cache_policy": "metadata-and-bytes", "sync_policy": "manual", "object_id": "object:webspace:operator-mutable-meta", "head_id": "head:webspace:operator-mutable-meta", "cache_state": "content_cached", "sync_state": "manual_idle", "kind": "metadata", "readonly": true, "access_policy": "resolver-readonly" },
                    { "name": "Folder", "is_file": false, "is_dir": true, "size": 0, "target_uri": "operator://drive/Writable/Folder", "resolver": "operator-drive", "resolver_state": "materialized-local", "cache_policy": "metadata-and-bytes", "sync_policy": "manual", "object_id": "object:webspace:operator-mutable-folder", "head_id": "head:webspace:operator-mutable-folder", "cache_state": "content_cached", "sync_state": "manual_idle", "kind": "materialized-directory", "readonly": false, "access_policy": "owner-writable" }
                ]
            })),
            Some("list") if path == "localhost://WebSpaces/Mutable" => Ok(json!({
                "status": "ok",
                "data": [
                    { "name": "_meta.json", "is_file": true, "is_dir": false, "size": 96, "resolver": "local-materialized", "cache_policy": "metadata-and-bytes", "sync_policy": "manual", "object_id": "object:webspace:mutable-meta", "head_id": "head:webspace:mutable-meta", "cache_state": "content_cached", "sync_state": "manual_idle", "kind": "metadata", "readonly": true, "access_policy": "resolver-readonly" },
                    { "name": "Folder", "is_file": false, "is_dir": true, "size": 0, "target_uri": "local://mutable/Folder", "resolver": "local-materialized", "resolver_state": "materialized-local", "cache_policy": "metadata-and-bytes", "sync_policy": "manual", "object_id": "object:webspace:mutable-folder", "head_id": "head:webspace:mutable-folder", "cache_state": "content_cached", "sync_state": "manual_pending", "kind": "materialized-directory", "readonly": false, "access_policy": "owner-writable" }
                ]
            })),
            Some("list") if path == "localhost://WebSpaces/Mutable/Folder" => Ok(json!({
                "status": "ok",
                "data": [
                    { "name": "_meta.json", "is_file": true, "is_dir": false, "size": 96, "resolver": "local-materialized", "cache_policy": "metadata-and-bytes", "sync_policy": "manual", "object_id": "object:webspace:mutable-folder-meta", "head_id": "head:webspace:mutable-folder-meta", "cache_state": "content_cached", "sync_state": "manual_idle", "kind": "metadata", "readonly": true, "access_policy": "resolver-readonly" },
                    { "name": "note.txt", "is_file": true, "is_dir": false, "size": 13, "target_uri": "local://mutable/Folder/note.txt", "resolver": "local-materialized", "resolver_state": "materialized-local", "cache_policy": "metadata-and-bytes", "sync_policy": "manual", "object_id": "object:webspace:mutable-note", "head_id": "head:webspace:mutable-note", "cache_state": "content_cached", "sync_state": "manual_pending", "kind": "materialized-file", "readonly": false, "access_policy": "owner-writable" }
                ]
            })),
            Some("list") if path == "localhost://WebSpaces/Elastos/content" => Ok(json!({
                "status": "ok",
                "data": [
                    {
                        "name": TEST_CIDV1,
                        "is_file": true,
                        "is_dir": false,
                        "size": 128,
                        "target_uri": format!("elastos://{TEST_CIDV1}"),
                        "provider": "content-provider",
                        "resolver_state": "resolved",
                        "resolver": "builtin",
                        "cache_policy": "metadata-only",
                        "sync_policy": "manual",
                        "object_id": "object:webspace:content-test-cid",
                        "head_id": "head:webspace:content-test-cid",
                        "cache_state": "metadata_cached",
                        "sync_state": "manual_idle",
                        "kind": "file-endpoint",
                        "readonly": true
                    }
                ]
            })),
            Some("stat") => {
                let stat = self
                    .cached_object(path)
                    .map(|cached| mock_cached_webspace_stat(path, &cached))
                    .unwrap_or_else(|| mock_webspace_stat(path));
                Ok(json!({
                    "status": "ok",
                    "data": stat
                }))
            }
            Some("health") => Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.webspace.health/v1",
                    "state": "metadata_ready",
                    "mounts": [
                        {
                            "moniker": "Cloud",
                            "resolver": "cloud-drive",
                            "live_adapter": true,
                            "adapter_state": "connected",
                            "adapter": {
                                "schema": "elastos.webspace.adapter/v1",
                                "resolver": "cloud-drive",
                                "provider": "cloud-drive-adapter",
                                "state": "connected",
                                "live": true,
                                "capabilities": ["metadata_index", "read_bytes"]
                            }
                        },
                        {
                            "moniker": "Operator",
                            "resolver": "operator-drive",
                            "live_adapter": true,
                            "adapter_state": "connected",
                            "adapter": {
                                "schema": "elastos.webspace.adapter/v1",
                                "resolver": "operator-drive",
                                "provider": "operator-drive-adapter",
                                "state": "connected",
                                "live": true,
                                "capabilities": ["metadata_index", "read_bytes", "write_bytes"]
                            }
                        },
                        {
                            "moniker": "OperatorMutable",
                            "resolver": "operator-drive",
                            "live_adapter": true,
                            "adapter_state": "connected",
                            "adapter": {
                                "schema": "elastos.webspace.adapter/v1",
                                "resolver": "operator-drive",
                                "provider": "operator-drive-adapter",
                                "state": "connected",
                                "live": true,
                                "capabilities": ["metadata_index", "read_bytes", "write_bytes"]
                            }
                        }
                    ]
                }
            })),
            Some("refresh") => Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.webspace.refresh-receipt/v1",
                    "action": "refreshed",
                    "handle_uri": path,
                    "byte_materialized": false
                }
            })),
            Some("cache") => {
                let cached = self.store_cached_object(path, request)?;
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "schema": "elastos.webspace.cache-receipt/v1",
                        "action": "content_cached",
                        "handle_uri": path,
                        "content_cached": true,
                        "dirty": false,
                        "size": cached.bytes.len()
                    }
                }))
            }
            Some("write")
                if path.starts_with("localhost://WebSpaces/Mutable/")
                    || path.starts_with("localhost://WebSpaces/OperatorMutable/") =>
            {
                let cached = self.store_written_object(path, request)?;
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "schema": "elastos.webspace.write-receipt/v1",
                        "action": "written",
                        "handle_uri": path,
                        "byte_materialized": true,
                        "size": cached.bytes.len()
                    }
                }))
            }
            Some("mkdir")
                if path.starts_with("localhost://WebSpaces/Mutable/")
                    || path.starts_with("localhost://WebSpaces/OperatorMutable/") =>
            {
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "schema": "elastos.webspace.mkdir-receipt/v1",
                        "action": "created",
                        "handle_uri": path
                    }
                }))
            }
            Some("delete")
                if path.starts_with("localhost://WebSpaces/Mutable/")
                    || path.starts_with("localhost://WebSpaces/OperatorMutable/") =>
            {
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "schema": "elastos.webspace.delete-receipt/v1",
                        "action": "deleted",
                        "handle_uri": path,
                        "removed_count": 1
                    }
                }))
            }
            Some("sync")
                if path.starts_with("localhost://WebSpaces/Mutable/")
                    || path.starts_with("localhost://WebSpaces/OperatorMutable/") =>
            {
                let synced = self.mark_synced(path)?.ok_or_else(|| {
                    ProviderError::Provider("mock sync target was not materialized".into())
                })?;
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "schema": "elastos.webspace.sync-receipt/v1",
                        "action": "resolver_synced",
                        "handle_uri": path,
                        "content_synced": true,
                        "dirty": false,
                        "size": synced.bytes.len()
                    }
                }))
            }
            Some("write" | "mkdir" | "delete") => Ok(json!({
                "status": "error",
                "code": "readonly",
                "message": "built-in or readonly WebSpace is resolver-owned and read-only"
            })),
            Some("read") if self.cached_object(path).is_some() => {
                let bytes = self.cached_object(path).unwrap().bytes;
                let size = bytes.len();
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "content": bytes,
                        "size": size
                    }
                }))
            }
            Some("read") if path.ends_with("_meta.json") || path.contains("/content/") => {
                let bytes = serde_json::to_vec_pretty(&json!({
                    "handle_uri": path.trim_end_matches("/_meta.json"),
                    "target_uri": if path.contains("/content/") {
                        Some(format!(
                            "elastos://{}",
                            path.rsplit('/').next().unwrap_or_default()
                        ))
                    } else {
                        None
                    },
                    "resolver_state": "resolved",
                    "resolver": "builtin",
                    "cache_policy": "metadata-only",
                    "sync_policy": "manual",
                    "object_id": "object:webspace:read",
                    "head_id": "head:webspace:read",
                    "cache_state": "metadata_cached",
                    "sync_state": "manual_idle"
                }))
                .unwrap();
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "content": bytes,
                        "size": bytes.len()
                    }
                }))
            }
            Some("read") if path.starts_with("localhost://WebSpaces/Mutable/") => {
                let bytes = b"mutable bytes".to_vec();
                let size = bytes.len();
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "content": bytes,
                        "size": size
                    }
                }))
            }
            Some(op) => Ok(json!({
                "status": "error",
                "code": "unsupported",
                "message": format!("unsupported mock WebSpace op: {op}")
            })),
            None => Ok(json!({
                "status": "error",
                "code": "invalid_request",
                "message": "missing mock WebSpace op"
            })),
        }
    }
}

#[async_trait::async_trait]
impl Provider for MockWebSpaceAdapterProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "mock WebSpace adapter provider only supports raw requests".into(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["cloud-drive-adapter"]
    }

    fn name(&self) -> &'static str {
        "mock-webspace-adapter-provider"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        if request.get("_runtime_invocation").is_none() {
            return Ok(json!({
                "status": "error",
                "code": "missing_runtime_invocation",
                "message": "WebSpace adapter requires Runtime provider invocation"
            }));
        }
        match request.get("op").and_then(|value| value.as_str()) {
            Some("metadata_index") => Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.webspace.adapter.metadata-index/v1",
                    "entries": [
                        {
                            "path": "Drive/Project X/file.pdf",
                            "kind": "file",
                            "target_uri": "cloud://drive/Drive/Project X/file.pdf",
                            "resolver_state": "indexed",
                            "readonly": true,
                            "description": "Adapter indexed Cloud Drive file."
                        }
                    ],
                    "receipt": {
                        "schema": "elastos.webspace.adapter.metadata-index-receipt/v1",
                        "resolver": "cloud-drive"
                    }
                }
            })),
            Some("read_bytes") => Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.webspace.adapter.read-bytes/v1",
                    "data": base64::engine::general_purpose::STANDARD.encode(b"cloud adapter bytes"),
                    "mime": "application/pdf",
                    "receipt": {
                        "schema": "elastos.webspace.adapter.read-bytes-receipt/v1",
                        "resolver": "cloud-drive",
                        "target_uri": request.get("target_uri").cloned()
                    }
                }
            })),
            Some(op) => Ok(json!({
                "status": "error",
                "code": "unsupported",
                "message": format!("unsupported mock WebSpace adapter op: {op}")
            })),
            None => Ok(json!({
                "status": "error",
                "code": "invalid_request",
                "message": "missing mock WebSpace adapter op"
            })),
        }
    }
}

#[async_trait::async_trait]
impl Provider for MockOperatorWebSpaceAdapterProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "mock operator WebSpace adapter only supports raw requests".into(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["operator-drive-adapter"]
    }

    fn name(&self) -> &'static str {
        "mock-operator-webspace-adapter"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        if request.get("_runtime_invocation").is_none() {
            return Ok(json!({
                "status": "error",
                "code": "missing_runtime_invocation",
                "message": "Operator WebSpace adapter requires Runtime provider invocation"
            }));
        }
        match request.get("op").and_then(|value| value.as_str()) {
            Some("metadata_index") => Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.webspace.adapter.metadata-index/v1",
                    "entries": [
                        {
                            "path": "Projects/Brief.md",
                            "kind": "file",
                            "target_uri": "operator://drive/Projects/Brief.md",
                            "resolver_state": "indexed",
                            "readonly": true,
                            "description": "Operator fixture indexed markdown brief."
                        },
                        {
                            "path": "Projects/Bundle.zip",
                            "kind": "file",
                            "target_uri": "operator://drive/Projects/Bundle.zip",
                            "resolver_state": "indexed",
                            "readonly": true,
                            "description": "Operator fixture indexed archive bundle."
                        }
                    ],
                    "receipt": {
                        "schema": "elastos.webspace.adapter.metadata-index-receipt/v1",
                        "resolver": "operator-drive",
                        "operator_fixture": true
                    }
                }
            })),
            Some("read_bytes") => {
                let target_uri = request
                    .get("target_uri")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                let (bytes, mime) = if target_uri.ends_with("/Bundle.zip") {
                    (mock_operator_archive_zip_bytes(), "application/zip")
                } else {
                    (
                        b"# Operator Brief\n\nAdapter-backed bytes.\n".to_vec(),
                        "text/plain",
                    )
                };
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "schema": "elastos.webspace.adapter.read-bytes/v1",
                        "data": base64::engine::general_purpose::STANDARD.encode(bytes),
                        "mime": mime,
                        "receipt": {
                            "schema": "elastos.webspace.adapter.read-bytes-receipt/v1",
                            "resolver": "operator-drive",
                            "operator_fixture": true,
                            "target_uri": request.get("target_uri").cloned()
                        }
                    }
                }))
            }
            Some("write_bytes") => {
                let target_uri = request
                    .get("target_uri")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                if target_uri.contains("Conflict") {
                    return Ok(json!({
                        "status": "error",
                        "code": "conflict",
                        "message": "operator fixture rejected stale mutable fork write",
                        "data": {
                            "schema": "elastos.webspace.adapter.write-conflict/v1",
                            "resolver": "operator-drive",
                            "target_uri": target_uri,
                            "reason": "head_mismatch"
                        }
                    }));
                }
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "schema": "elastos.webspace.adapter.write-bytes/v1",
                        "receipt": {
                            "schema": "elastos.webspace.adapter.write-bytes-receipt/v1",
                            "resolver": "operator-drive",
                            "operator_fixture": true,
                            "target_uri": target_uri,
                            "bytes_accepted": request
                                .get("data")
                                .and_then(serde_json::Value::as_str)
                                .and_then(|encoded| base64::engine::general_purpose::STANDARD.decode(encoded).ok())
                                .map(|bytes| bytes.len())
                                .unwrap_or(0)
                        }
                    }
                }))
            }
            Some(op) => Ok(json!({
                "status": "error",
                "code": "unsupported",
                "message": format!("unsupported mock operator WebSpace adapter op: {op}")
            })),
            None => Ok(json!({
                "status": "error",
                "code": "invalid_request",
                "message": "missing mock operator WebSpace adapter op"
            })),
        }
    }
}

fn mock_webspace_stat(path: &str) -> serde_json::Value {
    let is_cloud_file = path == "localhost://WebSpaces/Cloud/Drive/Project X/file.pdf";
    let is_operator_mutable = path.starts_with("localhost://WebSpaces/OperatorMutable");
    let is_operator = path.starts_with("localhost://WebSpaces/Operator") && !is_operator_mutable;
    let is_operator_file = path == "localhost://WebSpaces/Operator/Projects/Brief.md";
    let is_operator_archive_file = path == "localhost://WebSpaces/Operator/Projects/Bundle.zip";
    let is_operator_projects_dir = path == "localhost://WebSpaces/Operator/Projects";
    let is_mutable = path.starts_with("localhost://WebSpaces/Mutable");
    let is_mutable_file = is_mutable && path.ends_with(".txt");
    let is_operator_mutable_file =
        is_operator_mutable && (path.ends_with(".txt") || path.ends_with(".md"));
    let is_file = path.ends_with("_meta.json")
        || path.contains("/content/")
        || is_cloud_file
        || is_operator_file
        || is_operator_archive_file
        || is_operator_mutable_file
        || is_mutable_file;
    let is_cloud = path.starts_with("localhost://WebSpaces/Cloud");
    json!({
        "path": path,
        "is_file": is_file,
        "is_dir": !is_file,
        "size": if is_mutable_file { 13 } else if is_operator_mutable_file { 0 } else if is_file { 128 } else { 0 },
        "readonly": !(is_mutable || is_operator_mutable),
        "access_policy": if is_mutable || is_operator_mutable { "owner-writable" } else { "resolver-readonly" },
        "target_uri": if is_cloud_file {
            Some("cloud://drive/Drive/Project X/file.pdf".to_string())
        } else if is_operator_archive_file {
            Some("operator://drive/Projects/Bundle.zip".to_string())
        } else if is_cloud {
            Some(path.replacen("localhost://WebSpaces/Cloud", "cloud://drive", 1))
        } else if is_operator_mutable {
            Some(path.replacen("localhost://WebSpaces/OperatorMutable", "operator://drive/Writable", 1))
        } else if is_operator {
            Some(path.replacen("localhost://WebSpaces/Operator", "operator://drive", 1))
        } else if is_mutable {
            Some(path.replacen("localhost://WebSpaces/Mutable", "local://mutable", 1))
        } else if path.contains("/content/") {
            Some(format!(
                "elastos://{}",
                path.rsplit('/').next().unwrap_or_default()
            ))
        } else {
            None
        },
        "provider": if path.contains("/content/") { "content-provider" } else { "mock-webspace-provider" },
        "resolver_state": if is_mutable || is_operator_mutable {
            if path == "localhost://WebSpaces/Mutable" || path == "localhost://WebSpaces/OperatorMutable" { "mounted-mutable" } else { "materialized-local" }
        } else if is_operator_file || is_operator_archive_file || is_operator_projects_dir {
            "indexed"
        } else if is_operator {
            "indexed-virtual"
        } else if is_cloud_file { "indexed" } else if is_cloud { "indexed-virtual" } else { "resolved" },
        "resolver": if is_mutable { "local-materialized" } else if is_operator || is_operator_mutable { "operator-drive" } else if is_cloud { "cloud-drive" } else { "builtin" },
        "cache_policy": if is_mutable || is_operator || is_operator_mutable { "metadata-and-bytes" } else if is_cloud { "metadata-and-thumbnails" } else { "metadata-only" },
        "sync_policy": "manual",
        "object_id": format!("object:webspace:{}", path.replace('/', ":")),
        "head_id": format!("head:webspace:{}", path.replace('/', ":")),
        "cache_state": if is_mutable || is_operator_mutable { "content_cached" } else { "metadata_cached" },
        "sync_state": if is_mutable || is_operator_mutable { "manual_pending" } else { "manual_idle" },
        "kind": if path.ends_with("_meta.json") {
            "metadata"
        } else if is_mutable_file || is_operator_mutable_file {
            "materialized-file"
        } else if is_operator_mutable && path == "localhost://WebSpaces/OperatorMutable" {
            "mounted-webspace"
        } else if is_operator_mutable {
            "materialized-directory"
        } else if is_mutable && path == "localhost://WebSpaces/Mutable" {
            "mounted-webspace"
        } else if is_mutable {
            "materialized-directory"
        } else if is_operator_file || is_operator_archive_file {
            "indexed-file"
        } else if is_operator && path == "localhost://WebSpaces/Operator" {
            "mounted-webspace"
        } else if is_operator {
            "indexed-directory"
        } else if is_cloud_file {
            "indexed-file"
        } else if is_cloud {
            "indexed-directory"
        } else if path.contains("/content/") {
            "file-endpoint"
        } else if path == "localhost://WebSpaces" {
            "webspace-root"
        } else {
            "folder-handle"
        },
        "modified": 1,
        "created": 1
    })
}

fn mock_cached_webspace_stat(path: &str, cached: &MockCachedWebSpaceObject) -> serde_json::Value {
    let mut stat = mock_webspace_stat(path);
    stat["size"] = json!(cached.bytes.len());
    stat["resolver_state"] = json!("materialized-local");
    stat["cache_state"] = json!("content_cached");
    stat["sync_state"] = json!(cached.sync_state);
    stat["kind"] = json!("materialized-file");
    stat
}

struct MockNetProvider;
struct MockMalformedNetProvider;

#[async_trait::async_trait]
impl Provider for MockNetProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "mock net provider only supports raw requests".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["net"]
    }

    fn name(&self) -> &'static str {
        "mock-net"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        assert!(request
            .get("principal_id")
            .and_then(|value| value.as_str())
            .is_some());
        if request.get("op").and_then(|value| value.as_str()) == Some("status") {
            return Ok(json!({
                "status": "ok",
                "data": {
                    "provider": "net-provider",
                    "status": "fail_closed",
                    "direct_network": false,
                    "operations": ["resolve", "connect", "stream", "http"],
                    "exit_count": 0
                }
            }));
        }
        let operation = request
            .get("op")
            .and_then(|value| value.as_str())
            .unwrap_or("request");
        Ok(json!({
            "status": "error",
            "code": "exit_unavailable",
            "message": format!("No Browser Exit provider is configured for {operation}; net-provider refuses direct host networking")
        }))
    }
}

#[async_trait::async_trait]
impl Provider for MockMalformedNetProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "mock malformed net provider only supports raw requests".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["net"]
    }

    fn name(&self) -> &'static str {
        "mock-malformed-net"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        assert_eq!(
            request.get("op").and_then(|value| value.as_str()),
            Some("status")
        );
        Ok(json!({
            "status": "ok",
            "data": {
                "provider": "net-provider",
                "status": "exit_configured",
                "operations": ["resolve", "connect", "stream", "http"],
                "exit_count": 1
            }
        }))
    }
}

struct MockExitProvider;
struct MockMalformedExitProvider;
#[derive(Default)]
struct MockBrowserOwnershipCounts {
    launches: std::sync::atomic::AtomicUsize,
    first_video_frames: std::sync::atomic::AtomicUsize,
    active_pages: std::sync::atomic::AtomicUsize,
    active_vms: std::sync::atomic::AtomicUsize,
    active_streams: std::sync::atomic::AtomicUsize,
    active_routes: std::sync::atomic::AtomicUsize,
}

impl MockBrowserOwnershipCounts {
    fn observe_launch(&self) {
        use std::sync::atomic::Ordering;
        self.launches.fetch_add(1, Ordering::SeqCst);
        self.active_pages.fetch_add(1, Ordering::SeqCst);
        self.active_vms.fetch_add(1, Ordering::SeqCst);
        self.active_routes.fetch_add(1, Ordering::SeqCst);
    }

    fn observe_terminal_close(&self) {
        use std::sync::atomic::Ordering;
        for count in [&self.active_pages, &self.active_vms, &self.active_routes] {
            let _ = count.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
                value.checked_sub(1)
            });
        }
    }

    fn observe_stream_open(&self) {
        self.active_streams
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    fn observe_stream_close(&self) {
        let _ = self.active_streams.fetch_update(
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
            |value| value.checked_sub(1),
        );
    }

    fn snapshot(&self) -> (usize, usize, usize, usize, usize) {
        use std::sync::atomic::Ordering;
        (
            self.launches.load(Ordering::SeqCst),
            self.active_pages.load(Ordering::SeqCst),
            self.active_vms.load(Ordering::SeqCst),
            self.active_streams.load(Ordering::SeqCst),
            self.active_routes.load(Ordering::SeqCst),
        )
    }

    fn first_video_frame_count(&self) -> usize {
        self.first_video_frames
            .load(std::sync::atomic::Ordering::SeqCst)
    }
}

struct MockRemoteCarrierExitProvider {
    close_calls: BrowserCloseCallRecorder,
    close_failures_remaining: Arc<TokioMutex<usize>>,
    close_hangs_remaining: std::sync::atomic::AtomicUsize,
    close_started: Option<Arc<tokio::sync::Notify>>,
    ownership: Option<Arc<MockBrowserOwnershipCounts>>,
}

impl MockRemoteCarrierExitProvider {
    fn with_close_behavior(
        close_plan: MockExitClosePlan,
        ownership: Option<Arc<MockBrowserOwnershipCounts>>,
    ) -> Self {
        Self {
            close_calls: close_plan.close_calls,
            close_failures_remaining: Arc::new(TokioMutex::new(close_plan.close_failures)),
            close_hangs_remaining: std::sync::atomic::AtomicUsize::new(close_plan.close_hangs),
            close_started: close_plan.close_started,
            ownership,
        }
    }

    fn with_close_failures(close_calls: BrowserCloseCallRecorder, close_failures: usize) -> Self {
        Self::with_close_behavior(
            MockExitClosePlan {
                close_calls,
                close_failures,
                close_hangs: 0,
                close_started: None,
            },
            None,
        )
    }
}

#[async_trait::async_trait]
impl Provider for MockExitProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "mock exit provider only supports raw requests".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["exit"]
    }

    fn name(&self) -> &'static str {
        "mock-exit"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        assert!(request
            .get("principal_id")
            .and_then(|value| value.as_str())
            .is_some());
        if request.get("op").and_then(|value| value.as_str()) == Some("http_fetch") {
            return Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.exit.http-fetch.result/v1",
                    "backend": "mock-exit",
                    "url": request.get("url").cloned().unwrap_or_else(|| json!("")),
                    "method": request.get("method").cloned().unwrap_or_else(|| json!("GET")),
                    "body_text": "mock exit body",
                    "body_bytes": 14,
                    "body_truncated": false,
                    "status_code": 200
                }
            }));
        }
        if request.get("op").and_then(|value| value.as_str()) == Some("open_stream") {
            return Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.exit.stream-session/v1",
                    "backend": "mock-exit",
                    "stream_id": "stream:mock-exit:test",
                    "target": request.get("target").cloned().unwrap_or_else(|| json!("")),
                    "engine_owns_tls": true,
                    "state": "reserved",
                    "byte_transport": "not_attached"
                }
            }));
        }
        Ok(json!({
            "status": "ok",
            "data": {
                "provider": "exit-provider",
                "status": "fail_closed",
                "direct_network": false,
                "operations": ["quote", "open_stream", "close_stream", "http_fetch"],
                "backend_count": 0
            }
        }))
    }
}

#[async_trait::async_trait]
impl Provider for MockMalformedExitProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "mock malformed exit provider only supports raw requests".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["exit"]
    }

    fn name(&self) -> &'static str {
        "mock-malformed-exit"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        assert_eq!(
            request.get("op").and_then(|value| value.as_str()),
            Some("status")
        );
        Ok(json!({
            "status": "ok",
            "data": {
                "provider": "exit-provider",
                "status": "backend_configured",
                "operations": ["quote", "open_stream", "close_stream", "http_fetch"],
                "backend_count": 1
            }
        }))
    }
}

#[async_trait::async_trait]
impl Provider for MockRemoteCarrierExitProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "mock remote-carrier exit provider only supports raw requests".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["exit"]
    }

    fn name(&self) -> &'static str {
        "mock-remote-carrier-exit"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        let principal_id = request
            .get("principal_id")
            .and_then(|value| value.as_str())
            .unwrap_or("did:elastos:test");
        if request.get("op").and_then(|value| value.as_str()) == Some("close_stream") {
            // Append the exact call, then publish the new count, so a waiter
            // that sees the count can read this call.
            self.close_calls.record(request.clone()).await;
            if let Some(close_started) = &self.close_started {
                close_started.notify_one();
            }
            let should_hang = self
                .close_hangs_remaining
                .fetch_update(
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                    |remaining| remaining.checked_sub(1),
                )
                .is_ok();
            if should_hang {
                std::future::pending::<()>().await;
                unreachable!("hanging Exit close must be cancelled by Runtime timeout");
            }
            let mut failures_remaining = self.close_failures_remaining.lock().await;
            if *failures_remaining > 0 {
                *failures_remaining -= 1;
                return Ok(json!({
                    "status": "error",
                    "code": "close_stream_failed",
                    "message": "simulated remote Carrier Exit close_stream failure"
                }));
            }
            if let Some(ownership) = &self.ownership {
                ownership.observe_stream_close();
            }
            return Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.exit.close-stream/v1",
                    "closed": true,
                    "stream_id": request.get("stream_id").cloned().unwrap_or_else(|| json!("")),
                    "principal_id": request.get("principal_id").cloned().unwrap_or(serde_json::Value::Null),
                    "byte_transport": "carrier_stream"
                }
            }));
        }
        if request.get("op").and_then(|value| value.as_str()) == Some("open_stream") {
            if request
                .get("remote_exit_id")
                .and_then(|value| value.as_str())
                .is_some_and(|value| value != "mock-remote-carrier-exit")
            {
                return Ok(json!({
                    "status": "error",
                    "code": "exit_policy_blocked",
                    "message": "selected mock Remote Carrier Exit is not available"
                }));
            }
            let stream_id = request
                .get("stream_nonce")
                .and_then(|value| value.as_str())
                .filter(|value| {
                    !value.is_empty()
                        && value.bytes().all(|byte| {
                            byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_')
                        })
                })
                .map(|nonce| format!("remote-carrier:mock:test:{nonce}"))
                .unwrap_or_else(|| "remote-carrier:mock:test".to_string());
            if let Some(ownership) = &self.ownership {
                ownership.observe_stream_open();
            }
            return Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.exit.remote-carrier-session/v1",
                    "backend": "mock-remote-carrier-exit",
                    "grant_id": "operator-grant:mock-remote-carrier-exit:test",
                    "stream_id": stream_id,
                    "target": request.get("target").cloned().unwrap_or_else(|| json!("")),
                    "principal_id": principal_id,
                    "reason": request.get("reason").cloned().unwrap_or(serde_json::Value::Null),
                    "scheme": "tls",
                    "host": "glidefinance.io",
                    "engine_owns_tls": true,
                    "state": "reserved",
                    "byte_transport": "carrier_stream",
                    "carrier": {
                        "schema": "elastos.exit.remote-carrier/v1",
                        "peer_did": "did:elastos:remote-exit",
                        "carrier_service": "elastos://exit/open_stream",
                        "grant_id": "operator-grant:mock-remote-carrier-exit:test",
                        "transport": "carrier_stream",
                        "connect_ticket": "mock-private-carrier-connect-ticket"
                    },
                    "accounting": {
                        "grant_id": "operator-grant:mock-remote-carrier-exit:test",
                        "principal_id": principal_id,
                        "active_streams": 1,
                        "max_concurrent_streams": 2,
                        "egress_bytes": 0,
                        "ingress_bytes": 0
                    }
                }
            }));
        }
        Ok(json!({
            "status": "ok",
            "data": {
                "provider": "exit-provider",
                "status": "backend_configured",
                "direct_network": false,
                "operations": ["discover_remote_carrier_exits", "quote", "open_stream", "close_stream"],
                "backend_count": 0,
                "remote_carrier_exit_count": 1,
                "remote_carrier_exits": [{
                    "id": "mock-remote-carrier-exit",
                    "grant_id": "operator-grant:mock-remote-carrier-exit:test",
                    "peer_did": "did:elastos:remote-exit",
                    "carrier_service": "elastos://exit/open_stream",
                    "carrier": {
                        "schema": "elastos.exit.remote-carrier/v1",
                        "peer_did": "did:elastos:remote-exit",
                        "carrier_service": "elastos://exit/open_stream",
                        "grant_id": "operator-grant:mock-remote-carrier-exit:test",
                        "transport": "carrier_stream",
                        "connect_ticket": "mock-private-carrier-connect-ticket"
                    },
                    "allowed_for_principal": true,
                    "transport": "carrier_stream"
                }]
            }
        }))
    }
}

struct MockAttachedExitProvider {
    relay_ipc_path: Option<String>,
    stream_id: String,
}

struct MockPolicyBlockedExitProvider;

#[async_trait::async_trait]
impl Provider for MockPolicyBlockedExitProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "mock policy-blocked exit provider only supports raw requests".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["exit"]
    }

    fn name(&self) -> &'static str {
        "mock-policy-blocked-exit"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        assert!(request
            .get("principal_id")
            .and_then(|value| value.as_str())
            .is_some());
        Ok(json!({
            "status": "error",
            "code": "exit_policy_blocked",
            "message": "No Browser Exit backend allows host whatismyip.com; exit-provider refuses direct host networking"
        }))
    }
}

#[async_trait::async_trait]
impl Provider for MockAttachedExitProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "mock attached exit provider only supports raw requests".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["exit"]
    }

    fn name(&self) -> &'static str {
        "mock-attached-exit"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        assert!(request
            .get("principal_id")
            .and_then(|value| value.as_str())
            .is_some());
        if request.get("op").and_then(|value| value.as_str()) == Some("open_stream") {
            let stream_id = request
                .get("stream_nonce")
                .and_then(|value| value.as_str())
                .filter(|value| {
                    !value.is_empty()
                        && value.bytes().all(|byte| {
                            byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_')
                        })
                })
                .map(|nonce| format!("{}:{nonce}", self.stream_id))
                .unwrap_or_else(|| self.stream_id.clone());
            let relay_stream_id = stream_id.clone();
            let relay_ipc = self.relay_ipc_path.as_ref().map(|path| {
                json!({
                    "schema": "elastos.exit.relay-ipc/v1",
                    "kind": "unix_socket",
                    "path": path,
                    "stream_id": relay_stream_id
                })
            });
            return Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.exit.stream-session/v1",
                    "backend": "mock-attached-exit",
                    "stream_id": stream_id,
                    "target": request.get("target").cloned().unwrap_or_else(|| json!("")),
                    "scheme": "tls",
                    "host": "glidefinance.io",
                    "engine_owns_tls": true,
                    "state": "reserved",
                    "byte_transport": "adapter_ipc",
                    "adapter_ipc": {
                        "schema": "elastos.adapter-ipc/v1",
                        "kind": "unix_socket",
                        "path": "/tmp/elastos-browser-stream.sock",
                        "stream_id": stream_id
                    },
                    "relay_ipc": relay_ipc
                }
            }));
        }
        Ok(json!({
            "status": "ok",
            "data": {
                "provider": "exit-provider",
                "status": "ready",
                "direct_network": false,
                "operations": ["open_stream"],
                "backend_count": 1
            }
        }))
    }
}

fn mock_attached_stream_id(cache_dir: &std::path::Path) -> String {
    let digest = sha2::Sha256::digest(cache_dir.to_string_lossy().as_bytes());
    format!("stream:mock-attached-exit:{}", hex::encode(&digest[..4]))
}

struct MockBrowserEngineProvider;
struct MockRejectingBrowserEngineProvider;
struct MockMalformedBrowserEngineProvider;

#[derive(Clone, Copy)]
enum MockDispatchedBrowserLaunchFailure {
    ResponseLoss,
    MalformedSuccess,
    PendingThenTerminal,
    PendingThenLateSuccess,
    TransientThenLateSuccess,
    TimeoutThenLateSuccess,
    ImmediateLateSuccess,
    LateSuccessCleanupRetry,
    AlwaysUnavailable,
    HangingReconciliation,
    DidNotActResourcesInUse,
    ExactVzDidNotAct,
    MismatchedVzDidNotAct,
    TerminalVzSettlement,
    MismatchedTerminalVzSettlement,
}

struct MockReconciliatingBrowserEngineProvider {
    failure: MockDispatchedBrowserLaunchFailure,
    effect: TokioMutex<Option<serde_json::Value>>,
    launch_calls: std::sync::atomic::AtomicUsize,
    close_calls: BrowserCloseCallRecorder,
    reconciliation_calls: BrowserReconciliationCallRecorder,
}

#[derive(Clone, Copy)]
enum MockBrowserEngineCloseFailure {
    Transport,
    Adapter,
    AlreadyClosed,
}

struct MockRetryingBrowserEngineProvider {
    close_calls: Arc<TokioMutex<Vec<serde_json::Value>>>,
    close_failures_remaining: std::sync::atomic::AtomicUsize,
    failure: MockBrowserEngineCloseFailure,
    ownership: Option<Arc<MockBrowserOwnershipCounts>>,
}

struct MockForeignIdentityBrowserEngineProvider {
    close_calls: Arc<TokioMutex<Vec<serde_json::Value>>>,
}

impl MockRetryingBrowserEngineProvider {
    fn new(
        close_calls: Arc<TokioMutex<Vec<serde_json::Value>>>,
        failure: MockBrowserEngineCloseFailure,
        close_failures: usize,
    ) -> Self {
        Self {
            close_calls,
            close_failures_remaining: std::sync::atomic::AtomicUsize::new(close_failures),
            failure,
            ownership: None,
        }
    }

    fn with_ownership(
        close_calls: Arc<TokioMutex<Vec<serde_json::Value>>>,
        failure: MockBrowserEngineCloseFailure,
        close_failures: usize,
        ownership: Arc<MockBrowserOwnershipCounts>,
    ) -> Self {
        Self {
            close_calls,
            close_failures_remaining: std::sync::atomic::AtomicUsize::new(close_failures),
            failure,
            ownership: Some(ownership),
        }
    }
}

fn mock_browser_launch_page_id(request: &serde_json::Value) -> String {
    let url = request
        .get("url")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let reason = request
        .get("reason")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    if url.is_empty() && reason.is_empty() {
        return "page:mock-browser-engine".to_string();
    }
    if reason.contains("simulate close failure") {
        return "page:mock-browser-close-fails".to_string();
    }
    let digest = sha2::Sha256::digest(format!("{url}:{reason}").as_bytes());
    format!("page:mock-browser-engine-{}", hex::encode(&digest[..4]))
}

fn mock_browser_requested_page_id(request: &serde_json::Value) -> String {
    request
        .get("page_id")
        .and_then(|value| value.as_str())
        .unwrap_or("page:mock-browser-engine")
        .to_string()
}

fn mock_browser_terminal_cleanup_response(request: &serde_json::Value) -> serde_json::Value {
    let binding = request
        .get("runtime_cleanup")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    json!({
        "status": "ok",
        "data": {
            "schema": "elastos.browser.engine-cleanup-result/v2",
            "page_id": binding.get("page_id").cloned().unwrap_or(serde_json::Value::Null),
            "generation": binding.get("generation").cloned().unwrap_or(serde_json::Value::Null),
            "binding": binding,
            "terminal": true,
            "effects": {
                "page_absent": true,
                "child_absent": true,
                "vm_absent": true,
                "route_absent": true,
                "socket_absent": true
            }
        }
    })
}

fn assert_browser_close_request_contract(request: &serde_json::Value) {
    let request = request
        .as_object()
        .expect("Browser close request must be an object");
    assert_eq!(
        request.len(),
        4,
        "Browser close request must match the adapter's strict shape"
    );
    assert_eq!(
        request.get("op").and_then(serde_json::Value::as_str),
        Some("close_page")
    );
    assert!(request
        .get("page_id")
        .and_then(serde_json::Value::as_str)
        .is_some());
    assert!(request
        .get("principal_id")
        .and_then(serde_json::Value::as_str)
        .is_some());
    assert!(request
        .get("runtime_cleanup")
        .is_some_and(serde_json::Value::is_object));
}

#[async_trait::async_trait]
impl Provider for MockBrowserEngineProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "mock browser engine only supports raw requests".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["browser-engine"]
    }

    fn name(&self) -> &'static str {
        "mock-browser-engine"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        assert!(request
            .get("principal_id")
            .and_then(|value| value.as_str())
            .is_some());
        if request.get("op").and_then(|value| value.as_str()) == Some("status")
            && request.get("lifecycle_generation").is_some()
        {
            return Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.browser.engine.launch-reconciliation/v1",
                    "state": "did_not_act",
                    "lifecycle_generation": request["lifecycle_generation"],
                    "stream_id": request["stream_id"],
                    "effects": {
                        "page_acquired": false,
                        "vm_acquired": false,
                    },
                }
            }));
        }
        if request.get("op").and_then(|value| value.as_str()) == Some("launch") {
            let profile = request
                .get("profile")
                .and_then(|value| value.as_object())
                .expect("Browser launch must include a profile descriptor");
            assert_eq!(
                profile.get("schema").and_then(|value| value.as_str()),
                Some("elastos.browser.profile/v1")
            );
            assert_eq!(
                profile.get("scope").and_then(|value| value.as_str()),
                Some("active_principal")
            );
            assert_eq!(
                profile.get("storage").and_then(|value| value.as_str()),
                Some("principal_owned_profile_disk")
            );
            assert_eq!(
                profile
                    .get("storage_posture")
                    .and_then(|value| value.as_str()),
                Some("principal_owned_reset_scoped_unprotected")
            );
            assert_eq!(
                profile
                    .get("protected_storage")
                    .and_then(|value| value.as_bool()),
                Some(false)
            );
            assert_eq!(
                profile.get("encrypted").and_then(|value| value.as_bool()),
                Some(false)
            );
            assert_eq!(
                profile.get("recoverable").and_then(|value| value.as_bool()),
                Some(false)
            );
            assert_eq!(
                profile.get("recovery").and_then(|value| value.as_str()),
                Some("not_recovery_kit_packaged")
            );
            assert_eq!(
                profile.get("public_uri").and_then(|value| value.as_str()),
                Some("localhost://Users/self/BrowserProfiles/default/profile.ext4")
            );
            assert!(profile
                .get("uri")
                .and_then(|value| value.as_str())
                .is_some_and(|uri| {
                    uri.starts_with("localhost://Users/")
                        && uri.ends_with("/BrowserProfiles/default/profile.ext4")
                }));
            assert!(profile
                .get("profile_key")
                .and_then(|value| value.as_str())
                .is_some_and(|key| key.starts_with("profile-") && key.len() == 72));
            assert!(profile
                .get("disk_path")
                .and_then(|value| value.as_str())
                .is_some_and(|path| {
                    path.starts_with('/') && path.ends_with("/BrowserProfiles/default/profile.ext4")
                }));
            let stream_session = request
                .get("stream_session")
                .cloned()
                .unwrap_or_else(|| json!({}));
            assert_eq!(
                stream_session
                    .get("schema")
                    .and_then(|value| value.as_str()),
                Some("elastos.exit.stream-session/v1")
            );
            if stream_session
                .get("byte_transport")
                .and_then(|value| value.as_str())
                != Some("adapter_ipc")
            {
                return Ok(json!({
                    "status": "error",
                    "code": "byte_transport_unavailable",
                    "message": "Browser Engine Adapter requires adapter_ipc"
                }));
            }
            assert_eq!(
                stream_session
                    .get("adapter_ipc")
                    .and_then(|value| value.get("schema"))
                    .and_then(|value| value.as_str()),
                Some("elastos.adapter-ipc/v1")
            );
            if let Some(relay_ipc) = stream_session.get("relay_ipc") {
                assert_eq!(
                    relay_ipc.get("schema").and_then(|value| value.as_str()),
                    Some("elastos.exit.relay-ipc/v1")
                );
                assert_eq!(
                    relay_ipc.get("kind").and_then(|value| value.as_str()),
                    Some("unix_socket")
                );
                assert!(relay_ipc
                    .get("path")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .starts_with('/'));
            }
            let runtime_stream_path = stream_session
                .get("adapter_ipc")
                .and_then(|value| value.get("runtime_stream_path"))
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            let adapter_ipc_path = stream_session
                .get("adapter_ipc")
                .and_then(|value| value.get("path"))
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            assert!(adapter_ipc_path.starts_with('/'));
            assert!(adapter_ipc_path.ends_with(".sock"));
            assert!(runtime_stream_path.contains("elastos-browser-streams"));
            assert!(runtime_stream_path.ends_with(".sock"));
            assert_ne!(adapter_ipc_path, runtime_stream_path);
            let viewport = request
                .get("viewport")
                .filter(|value| value.is_object())
                .cloned()
                .unwrap_or_else(|| json!({"width": 1280, "height": 720}));
            let display_mode = request
                .get("display_mode")
                .and_then(|value| value.as_str())
                .unwrap_or("webrtc_remote_display");
            if display_mode != "webrtc_remote_display" {
                return Ok(json!({
                    "status": "error",
                    "code": "display_session_unavailable",
                    "message": format!("{display_mode} display sessions are unavailable in the mock browser engine")
                }));
            }
            if request
                .get("url")
                .and_then(|value| value.as_str())
                .is_some_and(|url| url.contains("slow-open.invalid"))
            {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
            if request
                .get("url")
                .and_then(|value| value.as_str())
                .is_some_and(|url| url.contains("capacity-unavailable.invalid"))
            {
                return Ok(json!({
                    "status": "error",
                    "code": "browser_capacity_unavailable",
                    "message": "Browser Engine Adapter has reached its active session limit (1)"
                }));
            }
            if request
                .get("url")
                .and_then(|value| value.as_str())
                .is_some_and(|url| url.contains("resources-in-use.invalid"))
            {
                return Ok(json!({
                    "status": "error",
                    "code": "resources_in_use",
                    "message": "Browser profile disk is already attached to an active VM"
                }));
            }
            let page_id = mock_browser_launch_page_id(request);
            let adapter = request
                .get("adapter_id")
                .and_then(|value| value.as_str())
                .unwrap_or("mock-browser-engine");
            let generation = request
                .get("lifecycle_generation")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            let stream_id = stream_session
                .get("stream_id")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            return Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.browser.engine.page/v1",
                    "provider": "browser-engine-adapter",
                    "protocol_version": "2.0",
                    "page_id": page_id,
                    "adapter": adapter,
                    "engine": "selkies_gstreamer",
                    "url": request.get("url").cloned().unwrap_or_else(|| json!("")),
                    "stream_id": stream_id,
                    "runtime_cleanup": {
                        "schema": "elastos.browser.engine-cleanup-binding/v2",
                        "page_id": page_id,
                        "generation": generation,
                        "stream_id": stream_id,
                        "adapter": adapter,
                        "engine": "selkies_gstreamer"
                    },
                    "network_mode": "runtime_net_only",
                    "direct_network": false,
                    "wallet_injection": false,
                    "display_session": {
                        "schema": "elastos.browser.display-session/v1",
                        "session_id": "display:mock-browser-engine",
                        "mode": "webrtc_remote_display",
                        "width": viewport["width"],
                        "height": viewport["height"],
                        "network_mode": "runtime_net_only",
                        "direct_network": false,
                        "input": "datachannel",
                        "input_protocol": "selkies_v1",
                        "display_backend": "selkies_gstreamer_webrtc",
                        "backend_class": "product_compositor",
                        "media_transport": "runtime_relay",
                        "audio": true,
                        "video": true,
                        "ice_servers": [{
                            "urls": ["turn:127.0.0.1:3478"],
                            "username_present": true,
                            "credential_present": true,
                            "credential_length": 16
                        }]
                    },
                    "view": {
                        "schema": "elastos.browser.view/v1",
                        "mode": "webrtc_remote_display",
                        "width": viewport["width"],
                        "height": viewport["height"]
                    }
                }
            }));
        }
        if request.get("op").and_then(|value| value.as_str()) == Some("page_status") {
            let page_id = mock_browser_requested_page_id(request);
            return Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.browser.page-status/v1",
                    "page_id": page_id,
                    "display_mode": "webrtc_remote_display",
                    "display_session": {
                        "schema": "elastos.browser.display-session/v1",
                        "session_id": "display:mock-browser-engine",
                        "mode": "webrtc_remote_display",
                        "width": 1280,
                        "height": 720,
                        "network_mode": "runtime_net_only",
                        "direct_network": false,
                        "input": "datachannel",
                        "input_protocol": "selkies_v1",
                        "display_backend": "selkies_gstreamer_webrtc",
                        "backend_class": "product_compositor",
                        "media_transport": "runtime_relay",
                        "audio": true,
                        "video": true,
                        "ice_servers": [{
                            "urls": ["turn:127.0.0.1:3478"],
                            "username_present": true,
                            "credential_present": true,
                            "credential_length": 16
                        }]
                    },
                    "actual_url": "https://glidefinance.io/",
                    "webrtc_connection_state": "connected",
                    "ice_connection_state": "connected",
                    "ice_gathering_state": "complete",
                    "direct_network": false
                }
            }));
        }
        if request.get("op").and_then(|value| value.as_str()) == Some("diagnostics") {
            let page_id = mock_browser_requested_page_id(request);
            return Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.browser.page-diagnostics/v1",
                    "page_id": page_id,
                    "url": "https://glidefinance.io/",
                    "title": "Glide",
                    "ready_state": "complete",
                    "viewport_width": 1280,
                    "viewport_height": 720,
                    "clickable_count": 1,
                    "clickable_elements": [{
                        "tag": "a",
                        "text": "Directory",
                        "aria_label": "",
                        "role": "",
                        "href": "https://glidefinance.io/directory",
                        "disabled": false,
                        "visible": true,
                        "rect": { "x": 48, "y": 96, "width": 120, "height": 32 }
                    }],
                    "image_count": 3,
                    "broken_image_count": 1,
                    "pending_image_count": 0,
                    "resource_count": 12,
                    "direct_network": false
                }
            }));
        }
        if request.get("op").and_then(|value| value.as_str()) == Some("input") {
            let page_id = mock_browser_requested_page_id(request);
            assert!(request.get("event").is_some());
            return Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.browser.input-result/v1",
                    "page_id": page_id,
                    "accepted": true
                }
            }));
        }
        if request.get("op").and_then(|value| value.as_str()) == Some("close_page") {
            assert_browser_close_request_contract(request);
            let page_id = mock_browser_requested_page_id(request);
            if page_id == "page:mock-browser-close-fails" {
                return Ok(json!({
                    "status": "error",
                    "code": "engine_process_unavailable",
                    "message": "simulated unreconciled close failure"
                }));
            }
            return Ok(mock_browser_terminal_cleanup_response(request));
        }
        if request.get("op").and_then(|value| value.as_str()) == Some("webrtc_signal") {
            let page_id = mock_browser_requested_page_id(request);
            let signal_schema = request
                .get("signal")
                .and_then(|value| value.get("schema"))
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            assert!(matches!(
                signal_schema,
                "elastos.browser.webrtc-offer/v1"
                    | "elastos.browser.webrtc-answer/v1"
                    | "elastos.browser.webrtc-candidate/v1"
                    | "elastos.browser.webrtc-end-of-candidates/v1"
            ));
            let signal_type = request
                .get("signal")
                .and_then(|value| value.get("type"))
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            if signal_type == "answer"
                || signal_type == "candidate"
                || signal_type == "end_of_candidates"
            {
                return Ok(json!({
                    "status": "ok",
                    "data": {
                        "schema": "elastos.browser.webrtc-signal-ack/v1",
                        "page_id": page_id,
                        "type": signal_type,
                        "accepted": true
                    }
                }));
            }
            assert_eq!(signal_type, "offer");
            if request
                .get("signal")
                .and_then(|value| value.get("sdp"))
                .and_then(|value| value.as_str())
                .is_some_and(|sdp| sdp.contains("simulate-provider-error"))
            {
                return Ok(json!({
                    "status": "error",
                    "code": "engine_process_unavailable",
                    "message": "browser page not found"
                }));
            }
            return Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.browser.webrtc-answer/v1",
                    "page_id": page_id,
                    "type": "answer",
                    "sdp": "v=0\r\ns=ElastOS Browser Test\r\n"
                }
            }));
        }
        Ok(json!({
            "status": "ok",
            "data": {
                "provider": "browser-engine-adapter",
                "protocol_version": "2.0",
                "status": "configured",
                "adapter_count": 2,
                "adapters": [
                    {
                        "id": "mock-browser-engine",
                        "engine": "selkies_gstreamer",
                        "default": true,
                        "backing_substrate": "operator_rbi",
                        "supported_display_modes": ["webrtc_remote_display"],
                        "supported_guarantee_levels": ["operator_rbi"],
                        "network_mode": "runtime_net_only",
                        "direct_network": false,
                        "wallet_injection": false
                    },
                    {
                        "id": "mock-jetson-engine",
                        "engine": "selkies_gstreamer",
                        "default": false,
                        "backing_substrate": "operator_rbi",
                        "supported_display_modes": ["webrtc_remote_display"],
                        "supported_guarantee_levels": ["operator_rbi"],
                        "network_mode": "runtime_net_only",
                        "direct_network": false,
                        "wallet_injection": false
                    }
                ],
                "direct_network": false,
                "wallet_injection": false,
                "stream_session_schema": "elastos.exit.stream-session/v1",
                "required_byte_transport": "adapter_ipc",
                "display_session_schema": "elastos.browser.display-session/v1",
                "supported_display_modes": ["webrtc_remote_display"],
                "supported_guarantee_levels": ["operator_rbi"],
                "operations": ["status", "launch", "attach_stream", "page_status", "diagnostics", "input", "webrtc_signal", "close_page"]
            }
        }))
    }
}

#[async_trait::async_trait]
impl Provider for MockReconciliatingBrowserEngineProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "mock reconciling browser engine only supports raw requests".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["browser-engine"]
    }

    fn name(&self) -> &'static str {
        "mock-browser-engine"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        match request.get("op").and_then(|value| value.as_str()) {
            Some("launch") => {
                let launch_call = self
                    .launch_calls
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if matches!(
                    self.failure,
                    MockDispatchedBrowserLaunchFailure::DidNotActResourcesInUse
                ) {
                    return Ok(json!({
                        "status": "error",
                        "code": "resources_in_use",
                        "message": "simulated Browser VM resource lease conflict",
                    }));
                }
                if matches!(
                    self.failure,
                    MockDispatchedBrowserLaunchFailure::ExactVzDidNotAct
                        | MockDispatchedBrowserLaunchFailure::MismatchedVzDidNotAct
                ) {
                    let adapter = request
                        .get("adapter_id")
                        .and_then(serde_json::Value::as_str)
                        .expect("Runtime must bind the default Adapter before VZ dispatch");
                    assert_eq!(adapter, "mock-browser-engine");
                    let authority = request
                        .get("transport_authority")
                        .expect("VZ settlement test transport authority");
                    let mut settlement = json!({
                        "schema": "elastos.browser.vz-launch-settlement/v1",
                        "state": "did_not_act",
                        "message": "injected exact VZ pre-effect failure",
                        "binding_hash": authority["binding_hash"],
                        "generation": authority["generation"],
                        "page_id": authority["page_id"],
                        "vm_id": authority["vm_id"],
                        "stream_id": authority["egress"]["stream_id"],
                        "media_stream_id": authority["media"]["stream_id"],
                        "effects": {
                            "session_directory": false,
                            "control_socket": false,
                            "ordinary_stream_bridge": false,
                            "media_stream_bridge": false,
                            "turn_process": false,
                            "supervisor_child": false,
                            "vm": false,
                        },
                        "absence": {
                            "child_absent": true,
                            "supervisor_child_absent": true,
                            "control_socket_absent": true,
                            "route_absent": true,
                            "turn_listener_absent": true,
                            "turn_relay_ports_absent": true,
                            "ordinary_stream_bridge_absent": true,
                            "media_stream_bridge_absent": true,
                            "session_directory_absent": true,
                            "vm_absent": true,
                        },
                    });
                    if matches!(
                        self.failure,
                        MockDispatchedBrowserLaunchFailure::MismatchedVzDidNotAct
                    ) {
                        settlement["vm_id"] = json!("vm:vz-substituted");
                    }
                    return Ok(json!({
                        "status": "error",
                        "code": "engine_process_unavailable",
                        "message": "injected VZ pre-effect failure",
                        "adapter": adapter,
                        "launch_settlement_result": settlement,
                    }));
                }
                if (matches!(
                    self.failure,
                    MockDispatchedBrowserLaunchFailure::PendingThenTerminal
                        | MockDispatchedBrowserLaunchFailure::PendingThenLateSuccess
                        | MockDispatchedBrowserLaunchFailure::TransientThenLateSuccess
                        | MockDispatchedBrowserLaunchFailure::TimeoutThenLateSuccess
                        | MockDispatchedBrowserLaunchFailure::TerminalVzSettlement
                        | MockDispatchedBrowserLaunchFailure::MismatchedTerminalVzSettlement
                ) && launch_call == 0)
                {
                    return Err(ProviderError::Provider(
                        "simulated browser-engine provider crash during launch".to_string(),
                    ));
                }
                let response = <MockBrowserEngineProvider as Provider>::send_raw(
                    &MockBrowserEngineProvider,
                    request,
                )
                .await?;
                let effect = response.get("data").cloned().expect("mock launch effect");
                *self.effect.lock().await = Some(effect);
                match self.failure {
                    MockDispatchedBrowserLaunchFailure::ResponseLoss => {
                        Err(ProviderError::Provider(
                            "simulated browser-engine launch response loss".to_string(),
                        ))
                    }
                    MockDispatchedBrowserLaunchFailure::MalformedSuccess => Ok(json!({
                        "status": "ok",
                        "data": {
                            "schema": "elastos.browser.engine.page/v1",
                            "provider": "browser-engine-adapter",
                            "protocol_version": "2.0",
                            "page_id": "unsafe page id",
                        }
                    })),
                    MockDispatchedBrowserLaunchFailure::PendingThenTerminal
                    | MockDispatchedBrowserLaunchFailure::PendingThenLateSuccess
                    | MockDispatchedBrowserLaunchFailure::TransientThenLateSuccess
                    | MockDispatchedBrowserLaunchFailure::TimeoutThenLateSuccess
                    | MockDispatchedBrowserLaunchFailure::ImmediateLateSuccess
                    | MockDispatchedBrowserLaunchFailure::LateSuccessCleanupRetry
                    | MockDispatchedBrowserLaunchFailure::AlwaysUnavailable
                    | MockDispatchedBrowserLaunchFailure::HangingReconciliation => Ok(response),
                    MockDispatchedBrowserLaunchFailure::DidNotActResourcesInUse => unreachable!(),
                    MockDispatchedBrowserLaunchFailure::ExactVzDidNotAct
                    | MockDispatchedBrowserLaunchFailure::MismatchedVzDidNotAct => unreachable!(),
                    MockDispatchedBrowserLaunchFailure::TerminalVzSettlement
                    | MockDispatchedBrowserLaunchFailure::MismatchedTerminalVzSettlement => {
                        Ok(response)
                    }
                }
            }
            Some("status") if request.get("lifecycle_generation").is_some() => {
                let reconciliation_call =
                    self.reconciliation_calls.record(request.clone()).await - 1;
                if matches!(
                    self.failure,
                    MockDispatchedBrowserLaunchFailure::DidNotActResourcesInUse
                ) {
                    return Ok(json!({
                        "status": "ok",
                        "data": {
                            "schema": "elastos.browser.engine.launch-reconciliation/v1",
                            "state": "did_not_act",
                            "lifecycle_generation": request["lifecycle_generation"],
                            "stream_id": request["stream_id"],
                            "effects": {
                                "page_acquired": false,
                                "vm_acquired": false,
                            },
                        }
                    }));
                }
                if matches!(
                    self.failure,
                    MockDispatchedBrowserLaunchFailure::MismatchedVzDidNotAct
                ) {
                    return Ok(json!({
                        "status": "ok",
                        "data": {
                            "schema": "elastos.browser.engine.launch-reconciliation/v1",
                            "state": "cleanup_pending",
                            "lifecycle_generation": request["lifecycle_generation"],
                            "stream_id": request["stream_id"],
                            "transport_authority": request["transport_authority"],
                        }
                    }));
                }
                if matches!(
                    self.failure,
                    MockDispatchedBrowserLaunchFailure::TerminalVzSettlement
                        | MockDispatchedBrowserLaunchFailure::MismatchedTerminalVzSettlement
                ) {
                    let authority = request
                        .get("transport_authority")
                        .expect("terminal VZ reconciliation authority");
                    let mut settlement = json!({
                        "schema": "elastos.browser.vz-launch-settlement/v1",
                        "state": "terminal_post_effect_cleanup",
                        "message": "injected exact terminal VZ cleanup",
                        "binding_hash": authority["binding_hash"],
                        "generation": authority["generation"],
                        "page_id": authority["page_id"],
                        "vm_id": authority["vm_id"],
                        "stream_id": authority["egress"]["stream_id"],
                        "media_stream_id": authority["media"]["stream_id"],
                        "effects": {
                            "session_directory": true,
                            "control_socket": true,
                            "ordinary_stream_bridge": true,
                            "media_stream_bridge": true,
                            "turn_process": true,
                            "supervisor_child": false,
                            "vm": true,
                        },
                        "absence": {
                            "child_absent": true,
                            "supervisor_child_absent": true,
                            "control_socket_absent": true,
                            "route_absent": true,
                            "turn_listener_absent": true,
                            "turn_relay_ports_absent": true,
                            "ordinary_stream_bridge_absent": true,
                            "media_stream_bridge_absent": true,
                            "session_directory_absent": true,
                            "vm_absent": true,
                        },
                    });
                    if matches!(
                        self.failure,
                        MockDispatchedBrowserLaunchFailure::MismatchedTerminalVzSettlement
                    ) {
                        settlement["media_stream_id"] = json!("stream:vz-media-substituted");
                    }
                    return Ok(json!({
                        "status": "ok",
                        "data": {
                            "schema": "elastos.browser.engine.launch-reconciliation/v1",
                            "state": "terminal_post_effect_cleanup",
                            "lifecycle_generation": request["lifecycle_generation"],
                            "stream_id": request["stream_id"],
                            "transport_authority": authority,
                            "effects": {
                                "page_acquired": false,
                                "vm_acquired": true,
                            },
                            "terminal_cleanup_receipt": settlement,
                        }
                    }));
                }
                if matches!(
                    self.failure,
                    MockDispatchedBrowserLaunchFailure::PendingThenTerminal
                ) {
                    if reconciliation_call < 2 {
                        return Ok(json!({
                            "status": "ok",
                            "data": {
                                "schema": "elastos.browser.engine.launch-reconciliation/v1",
                                "state": "cleanup_pending",
                                "lifecycle_generation": request["lifecycle_generation"],
                                "stream_id": request["stream_id"],
                            }
                        }));
                    }
                    return Ok(json!({
                        "status": "ok",
                        "data": {
                            "schema": "elastos.browser.engine.launch-reconciliation/v1",
                            "state": "terminal_post_effect_cleanup",
                            "lifecycle_generation": request["lifecycle_generation"],
                            "stream_id": request["stream_id"],
                            "effects": {
                                "page_acquired": true,
                                "vm_acquired": true,
                            },
                        }
                    }));
                }
                if matches!(
                    self.failure,
                    MockDispatchedBrowserLaunchFailure::AlwaysUnavailable
                ) {
                    return Err(ProviderError::Provider(
                        "simulated unavailable Browser reconciliation authority".to_string(),
                    ));
                }
                if matches!(
                    self.failure,
                    MockDispatchedBrowserLaunchFailure::HangingReconciliation
                ) {
                    return std::future::pending::<Result<serde_json::Value, ProviderError>>()
                        .await;
                }
                if matches!(
                    self.failure,
                    MockDispatchedBrowserLaunchFailure::TimeoutThenLateSuccess
                ) && reconciliation_call == 0
                {
                    return std::future::pending::<Result<serde_json::Value, ProviderError>>()
                        .await;
                }
                if matches!(
                    self.failure,
                    MockDispatchedBrowserLaunchFailure::TransientThenLateSuccess
                ) && reconciliation_call == 0
                {
                    return Err(ProviderError::Provider(
                        "simulated transient Browser reconciliation failure".to_string(),
                    ));
                }
                if matches!(
                    self.failure,
                    MockDispatchedBrowserLaunchFailure::PendingThenLateSuccess
                ) && reconciliation_call == 0
                {
                    return Ok(json!({
                        "status": "ok",
                        "data": {
                            "schema": "elastos.browser.engine.launch-reconciliation/v1",
                            "state": "cleanup_pending",
                            "lifecycle_generation": request["lifecycle_generation"],
                            "stream_id": request["stream_id"],
                        }
                    }));
                }
                if matches!(
                    self.failure,
                    MockDispatchedBrowserLaunchFailure::PendingThenLateSuccess
                        | MockDispatchedBrowserLaunchFailure::TransientThenLateSuccess
                        | MockDispatchedBrowserLaunchFailure::TimeoutThenLateSuccess
                        | MockDispatchedBrowserLaunchFailure::ImmediateLateSuccess
                        | MockDispatchedBrowserLaunchFailure::LateSuccessCleanupRetry
                ) {
                    let generation = request["lifecycle_generation"]
                        .as_str()
                        .expect("mock lifecycle generation");
                    let stream_id = request["stream_id"].as_str().expect("mock stream id");
                    let page_id = format!(
                        "page:late-effect:{}",
                        generation.trim_start_matches("sha256:")
                    );
                    return Ok(json!({
                        "status": "ok",
                        "data": {
                            "schema": "elastos.browser.engine.launch-reconciliation/v1",
                            "state": "effect_acquired",
                            "lifecycle_generation": generation,
                            "stream_id": stream_id,
                            "effect": {
                                "provider": "browser-engine-adapter",
                                "protocol_version": "2.0",
                                "page_id": page_id,
                                "adapter": "mock-browser-engine",
                                "engine": "selkies_gstreamer",
                                "stream_id": stream_id,
                                "runtime_cleanup": {
                                    "schema": "elastos.browser.engine-cleanup-binding/v2",
                                    "page_id": page_id,
                                    "generation": generation,
                                    "stream_id": stream_id,
                                    "adapter": "mock-browser-engine",
                                    "engine": "selkies_gstreamer",
                                    "control_socket_path": "/tmp/mock-browser-late-effect.sock"
                                }
                            }
                        }
                    }));
                }
                if let Some(effect) = self.effect.lock().await.clone() {
                    return Ok(json!({
                        "status": "ok",
                        "data": {
                            "schema": "elastos.browser.engine.launch-reconciliation/v1",
                            "state": "effect_acquired",
                            "lifecycle_generation": request["lifecycle_generation"],
                            "stream_id": request["stream_id"],
                            "effect": effect,
                        }
                    }));
                }
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "schema": "elastos.browser.engine.launch-reconciliation/v1",
                        "state": "cleanup_pending",
                        "lifecycle_generation": request["lifecycle_generation"],
                        "stream_id": request["stream_id"],
                    }
                }))
            }
            Some("close_page") => {
                assert_browser_close_request_contract(request);
                // Append the exact call, then publish the new count, so a
                // waiter that sees the count can read this call.
                let close_call = self.close_calls.record(request.clone()).await;
                if matches!(
                    self.failure,
                    MockDispatchedBrowserLaunchFailure::LateSuccessCleanupRetry
                ) && close_call <= 2
                {
                    return Ok(json!({
                        "status": "error",
                        "code": "engine_cleanup_indeterminate",
                        "message": "simulated nonterminal Browser cleanup"
                    }));
                }
                *self.effect.lock().await = None;
                Ok(mock_browser_terminal_cleanup_response(request))
            }
            _ => {
                <MockBrowserEngineProvider as Provider>::send_raw(
                    &MockBrowserEngineProvider,
                    request,
                )
                .await
            }
        }
    }
}

#[async_trait::async_trait]
impl Provider for MockRetryingBrowserEngineProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "mock retrying browser engine only supports raw requests".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["browser-engine"]
    }

    fn name(&self) -> &'static str {
        "mock-retrying-browser-engine"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        if request.get("op").and_then(|value| value.as_str()) == Some("launch") {
            let response = <MockBrowserEngineProvider as Provider>::send_raw(
                &MockBrowserEngineProvider,
                request,
            )
            .await?;
            if response.get("status").and_then(|value| value.as_str()) == Some("ok") {
                if let Some(ownership) = &self.ownership {
                    ownership.observe_launch();
                }
            }
            return Ok(response);
        }
        if request.get("op").and_then(|value| value.as_str()) == Some("close_page") {
            assert_browser_close_request_contract(request);
            self.close_calls.lock().await.push(request.clone());
            let should_fail = self
                .close_failures_remaining
                .fetch_update(
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                    |remaining| remaining.checked_sub(1),
                )
                .is_ok();
            if should_fail {
                return match self.failure {
                    MockBrowserEngineCloseFailure::Transport => Err(ProviderError::Provider(
                        "simulated browser-engine close transport failure".to_string(),
                    )),
                    MockBrowserEngineCloseFailure::Adapter => Ok(json!({
                        "status": "error",
                        "code": "engine_close_indeterminate",
                        "message": "simulated adapter close failure"
                    })),
                    MockBrowserEngineCloseFailure::AlreadyClosed => {
                        Ok(mock_browser_terminal_cleanup_response(request))
                    }
                };
            }
            let response = <MockBrowserEngineProvider as Provider>::send_raw(
                &MockBrowserEngineProvider,
                request,
            )
            .await?;
            if response.get("status").and_then(|value| value.as_str()) == Some("ok") {
                if let Some(ownership) = &self.ownership {
                    ownership.observe_terminal_close();
                }
            }
            return Ok(response);
        }
        <MockBrowserEngineProvider as Provider>::send_raw(&MockBrowserEngineProvider, request).await
    }
}

#[async_trait::async_trait]
impl Provider for MockForeignIdentityBrowserEngineProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "mock foreign-identity browser engine only supports raw requests".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["browser-engine"]
    }

    fn name(&self) -> &'static str {
        "mock-retrying-browser-engine"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        if request.get("op").and_then(|value| value.as_str()) == Some("close_page") {
            self.close_calls.lock().await.push(request.clone());
        }
        Ok(json!({
            "status": "ok",
            "data": {
                "provider": "foreign-browser-engine",
                "protocol_version": "9.9",
                "status": "configured",
                "direct_network": false,
                "wallet_injection": false
            }
        }))
    }
}

#[async_trait::async_trait]
impl Provider for MockRejectingBrowserEngineProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "mock rejecting browser engine only supports raw requests".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["browser-engine"]
    }

    fn name(&self) -> &'static str {
        "mock-rejecting-browser-engine"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        match request.get("op").and_then(|value| value.as_str()) {
            Some("launch") => Ok(json!({
                "status": "error",
                "code": "display_session_unavailable",
                "message": "Browser Engine Adapter rejected launch after stream reservation"
            })),
            Some("status") if request.get("lifecycle_generation").is_some() => Ok(json!({
                "status": "ok",
                "data": {
                    "schema": "elastos.browser.engine.launch-reconciliation/v1",
                    "state": "did_not_act",
                    "lifecycle_generation": request["lifecycle_generation"],
                    "stream_id": request["stream_id"],
                    "effects": {
                        "page_acquired": false,
                        "vm_acquired": false,
                    },
                }
            })),
            Some("status") => Ok(json!({
                "status": "ok",
                "data": {
                    "provider": "browser-engine-adapter",
                    "protocol_version": "2.0",
                    "status": "configured",
                    "adapter_count": 1,
                    "adapters": [{
                        "id": "mock-browser-engine",
                        "engine": "selkies_gstreamer",
                        "default": true,
                        "direct_network": false,
                        "wallet_injection": false
                    }],
                    "direct_network": false,
                    "wallet_injection": false,
                    "stream_session_schema": "elastos.exit.stream-session/v1",
                    "required_byte_transport": "adapter_ipc",
                    "display_session_schema": "elastos.browser.display-session/v1",
                    "supported_display_modes": ["webrtc_remote_display"],
                    "supported_guarantee_levels": ["operator_rbi"],
                    "operations": ["status", "launch"]
                }
            })),
            _ => Ok(json!({
                "status": "error",
                "code": "unsupported_operation",
                "message": "mock rejecting browser engine only supports status and launch"
            })),
        }
    }
}

#[async_trait::async_trait]
impl Provider for MockMalformedBrowserEngineProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "mock malformed browser engine only supports raw requests".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["browser-engine"]
    }

    fn name(&self) -> &'static str {
        "mock-malformed-browser-engine"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        assert_eq!(
            request.get("op").and_then(|value| value.as_str()),
            Some("status")
        );
        Ok(json!({
            "status": "ok",
            "data": {
                "provider": "browser-engine-adapter",
                "status": "configured",
                "adapter_count": 1,
                "required_byte_transport": "adapter_ipc",
                "stream_session_schema": "elastos.exit.stream-session/v1",
                "display_session_schema": "elastos.browser.display-session/v1",
                "supported_display_modes": ["webrtc_remote_display"]
            }
        }))
    }
}

#[derive(Default)]
struct MockWalletProvider {
    challenges: TokioMutex<HashMap<String, MockWalletChallenge>>,
    bitcoin_challenges: TokioMutex<HashMap<String, MockBitcoinChallenge>>,
    accounts: TokioMutex<Vec<serde_json::Value>>,
    approvals: TokioMutex<Vec<serde_json::Value>>,
    defaults: TokioMutex<Vec<serde_json::Value>>,
}

struct MockWalletChallenge {
    challenge: AuthChallengeV1,
    consumed: bool,
}

struct MockBitcoinChallenge {
    message: String,
    address: String,
    consumed: bool,
}

#[async_trait::async_trait]
impl Provider for MockWalletProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "mock wallet provider only supports raw requests".into(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["wallet"]
    }

    fn name(&self) -> &'static str {
        "mock-wallet-provider"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        if request.get("op").and_then(|value| value.as_str()) == Some(WALLET_BUS_OPERATION) {
            let request_bytes =
                serde_json::to_vec(request.get("request").ok_or_else(|| {
                    ProviderError::Provider("missing Wallet Bus v2 request".into())
                })?)
                .map_err(|err| ProviderError::Provider(err.to_string()))?;
            let wallet_request =
                WalletProviderRequestV2::decode_at(&request_bytes, crate::auth::now_ts())
                    .map_err(|err| ProviderError::Provider(err.to_string()))?;
            let legacy_request = match &wallet_request.operation {
                WalletProviderOperationV2::ListAccounts { include_revoked } => json!({
                    "op": "accounts",
                    "principal_id": wallet_request.authority.principal_id,
                    "include_revoked": include_revoked,
                }),
                WalletProviderOperationV2::CreateManagedAccount {
                    chain_namespace,
                    label,
                    create_new,
                } => json!({
                    "op": "create_managed_account",
                    "principal_id": wallet_request.authority.principal_id,
                    "chain_namespace": chain_namespace,
                    "label": label,
                    "create_new": create_new,
                }),
                WalletProviderOperationV2::RevokeAccount { account_id } => json!({
                    "op": "revoke_account",
                    "principal_id": wallet_request.authority.principal_id,
                    "account_id": account_id,
                }),
                WalletProviderOperationV2::RenameAccount { account_id, label } => json!({
                    "op": "rename_account",
                    "principal_id": wallet_request.authority.principal_id,
                    "account_id": account_id,
                    "label": label,
                }),
                WalletProviderOperationV2::SetDefaultAccount {
                    chain_namespace,
                    intent,
                    account_id,
                } => json!({
                    "op": "set_default_account",
                    "principal_id": wallet_request.authority.principal_id,
                    "chain_namespace": chain_namespace,
                    "intent": intent,
                    "account_id": account_id,
                }),
                WalletProviderOperationV2::ExportManagedRecoveryKey { account_id } => json!({
                    "op": "export_managed_secret",
                    "principal_id": wallet_request.authority.principal_id,
                    "account_id": account_id,
                }),
                WalletProviderOperationV2::ImportManagedRecoveryKey {
                    recovery_key,
                    label,
                } => json!({
                    "op": "import_managed_secret",
                    "principal_id": wallet_request.authority.principal_id,
                    "recovery_key": recovery_key,
                    "label": label,
                }),
                WalletProviderOperationV2::ExportManagedRecoverySet {} => json!({
                    "op": "export_managed_recovery_set",
                    "principal_id": wallet_request.authority.principal_id,
                }),
                WalletProviderOperationV2::ImportManagedRecoverySet { recovery_set } => json!({
                    "op": "import_managed_recovery_set",
                    "principal_id": wallet_request.authority.principal_id,
                    "recovery_set": recovery_set,
                }),
                WalletProviderOperationV2::Challenge {
                    domain,
                    uri,
                    address,
                    chain_id,
                    resources,
                } => json!({
                    "op": "challenge",
                    "domain": domain,
                    "uri": uri,
                    "address": address,
                    "chain_id": chain_id,
                    "resources": resources,
                }),
                WalletProviderOperationV2::BitcoinChallenge {
                    domain,
                    uri,
                    address,
                    network,
                    resources,
                } => json!({
                    "op": "bitcoin_challenge",
                    "domain": domain,
                    "uri": uri,
                    "address": address,
                    "network": network.as_str(),
                    "resources": resources,
                }),
                WalletProviderOperationV2::VerifyProof { message, signature } => json!({
                    "op": "verify_proof",
                    "message": message,
                    "signature": signature,
                }),
                WalletProviderOperationV2::VerifyContractProof {
                    message,
                    signature,
                    evidence,
                } => json!({
                    "op": "verify_contract_proof",
                    "message": message,
                    "signature": signature,
                    "erc1271_proof": evidence,
                }),
                WalletProviderOperationV2::VerifyBip322Proof {
                    message,
                    signature,
                    signature_type,
                    public_key,
                } => json!({
                    "op": "verify_bip322_proof",
                    "message": message,
                    "signature": signature,
                    "signature_type": signature_type,
                    "public_key": public_key,
                }),
                WalletProviderOperationV2::LinkVerifiedAccount {
                    proof_binding_id,
                    chain_namespace,
                    address,
                    proof_type,
                    ..
                } => json!({
                    "op": "link_account",
                    "principal_id": wallet_request.authority.principal_id,
                    "proof_binding_id": proof_binding_id,
                    "chain_namespace": chain_namespace,
                    "address": address,
                    "proof_type": proof_type,
                    "connector_id": wallet_request.authority.actor,
                }),
                WalletProviderOperationV2::RequestApproval {
                    account_id,
                    chain_namespace,
                    intent,
                    resource,
                    reason,
                    payload,
                    expires_at,
                } => json!({
                    "op": "request_signature",
                    "request_id": wallet_request.request_id,
                    "wallet_request_sha256": wallet_request.request_sha256,
                    "authority_binding": wallet_request.session_binding,
                    "principal_id": wallet_request.authority.principal_id,
                    "session_id": wallet_request.authority.session_id,
                    "launch_id": wallet_request.authority.launch_id,
                    "account_id": account_id,
                    "chain_namespace": chain_namespace,
                    "intent": intent,
                    "capsule_id": wallet_request.authority.actor,
                    "resource": resource,
                    "reason": reason,
                    "payload": payload,
                    "expires_at": expires_at,
                }),
                WalletProviderOperationV2::AttachValidatedChainOutcome { outcome } => json!({
                    "op": "attach_validated_chain_outcome",
                    "principal_id": wallet_request.authority.principal_id,
                    "session_id": wallet_request.authority.session_id,
                    "launch_id": wallet_request.authority.launch_id,
                    "capsule_id": wallet_request.authority.actor,
                    "outcome": outcome,
                }),
                WalletProviderOperationV2::ListApprovals { include_resolved } => json!({
                    "op": "approval_requests",
                    "principal_id": wallet_request.authority.principal_id,
                    "include_resolved": include_resolved,
                }),
                WalletProviderOperationV2::RejectApproval { request_id, reason } => json!({
                    "op": "reject_approval",
                    "principal_id": wallet_request.authority.principal_id,
                    "request_id": request_id,
                    "reason": reason,
                }),
                WalletProviderOperationV2::ApproveAndSignManaged { request_id, reason } => json!({
                    "op": "approve_and_sign_managed",
                    "principal_id": wallet_request.authority.principal_id,
                    "request_id": request_id,
                    "reason": reason,
                }),
                WalletProviderOperationV2::ApproveConnectorHandoff { request_id, reason } => {
                    json!({
                        "op": "approve_approval",
                        "principal_id": wallet_request.authority.principal_id,
                        "request_id": request_id,
                        "reason": reason,
                    })
                }
                WalletProviderOperationV2::CompleteConnectorHandoff {
                    request_id,
                    payload_hash,
                    signature,
                    signature_type,
                    public_key,
                    signer,
                    transaction_hash,
                } => json!({
                    "op": "complete_approval",
                    "principal_id": wallet_request.authority.principal_id,
                    "request_id": request_id,
                    "connector_id": wallet_request.authority.actor,
                    "payload_hash": payload_hash,
                    "signature": signature,
                    "signature_type": signature_type,
                    "public_key": public_key,
                    "signer": signer,
                    "transaction_hash": transaction_hash,
                }),
                _ => {
                    let response = WalletProviderResponseV2::for_request(
                        &wallet_request,
                        WalletResultV2::Error {
                            code: "unsupported_operation".to_string(),
                            message: "mock Wallet Bus v2 does not support this operation"
                                .to_string(),
                        },
                    );
                    return Ok(json!({"status": "ok", "data": response}));
                }
            };
            let legacy_response = self.send_legacy_raw(&legacy_request).await?;
            let result = match legacy_response.get("status").and_then(Value::as_str) {
                Some("ok") => WalletResultV2::Ok {
                    data: legacy_response.get("data").cloned().unwrap_or(Value::Null),
                },
                Some("error") => WalletResultV2::Error {
                    code: legacy_response
                        .get("code")
                        .and_then(Value::as_str)
                        .unwrap_or("provider_error")
                        .to_string(),
                    message: legacy_response
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("mock Wallet provider rejected the request")
                        .to_string(),
                },
                _ => {
                    return Err(ProviderError::Provider(
                        "mock Wallet provider returned malformed response".into(),
                    ));
                }
            };
            return Ok(json!({
                "status": "ok",
                "data": WalletProviderResponseV2::for_request(&wallet_request, result),
            }));
        }

        self.send_legacy_raw(request).await
    }
}

#[derive(Default)]
struct RecordingWalletProvider {
    provider: MockWalletProvider,
    requests: TokioMutex<Vec<serde_json::Value>>,
}

#[async_trait::async_trait]
impl Provider for RecordingWalletProvider {
    async fn handle(&self, request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        self.provider.handle(request).await
    }

    fn schemes(&self) -> Vec<&'static str> {
        self.provider.schemes()
    }

    fn name(&self) -> &'static str {
        "recording-mock-wallet-provider"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        self.requests.lock().await.push(request.clone());
        self.provider.send_raw(request).await
    }
}

impl RecordingWalletProvider {
    fn new(provider: MockWalletProvider) -> Self {
        Self {
            provider,
            requests: TokioMutex::default(),
        }
    }

    async fn assert_no_requests(&self) {
        let requests = self.requests.lock().await;
        assert!(
            requests.is_empty(),
            "request rejected at the Runtime boundary reached Wallet Provider: {requests:?}"
        );
    }

    async fn assert_v2_operations(
        &self,
        expected_actor: &str,
        expected_authority: &TestPasskeyAuthority,
        expected: &[WalletOperationKind],
    ) {
        let requests = self.requests.lock().await;
        let mut actual = Vec::with_capacity(requests.len());
        let mut launch_id = None;
        for request in requests.iter() {
            assert_eq!(
                request.get("op").and_then(Value::as_str),
                Some(WALLET_BUS_OPERATION),
                "migrated wallet-link route emitted a retired raw Wallet request: {request}"
            );
            let request_bytes = serde_json::to_vec(
                request
                    .get("request")
                    .expect("Wallet Bus v2 request envelope"),
            )
            .unwrap();
            let wallet_request =
                WalletProviderRequestV2::decode_at(&request_bytes, crate::auth::now_ts()).unwrap();
            assert_eq!(wallet_request.authority.actor, expected_actor);
            assert_eq!(
                wallet_request.authority.principal_id,
                expected_authority.principal_id
            );
            assert_eq!(
                wallet_request.authority.session_id,
                expected_authority.session_id
            );
            assert_eq!(
                wallet_request.authority.proof_binding_id.as_deref(),
                Some(expected_authority.proof_binding_id.as_str())
            );
            assert_eq!(
                wallet_request.authority.grant_id,
                expected_authority.grant_id
            );
            if let Some(expected_launch_id) = launch_id.as_deref() {
                assert_eq!(wallet_request.authority.launch_id, expected_launch_id);
            } else {
                launch_id = Some(wallet_request.authority.launch_id.clone());
            }
            actual.push(wallet_request.operation.kind());
        }
        assert_eq!(actual, expected);
    }

    async fn assert_v2_account_reads(
        &self,
        expected_authority: &RuntimeWalletAuthority,
        expected_count: usize,
    ) {
        let expected = vec![WalletOperationKind::ListAccounts; expected_count];
        self.assert_v2_account_operations(expected_authority, &expected)
            .await;
    }

    async fn assert_v2_account_operations(
        &self,
        expected_authority: &RuntimeWalletAuthority,
        expected_operations: &[WalletOperationKind],
    ) {
        let expected = expected_authority.verified_context();
        let requests = self.requests.lock().await;
        let mut actual = Vec::new();
        for request in requests.iter() {
            let operation = request.get("op").and_then(Value::as_str);
            assert!(
                !matches!(
                    operation,
                    Some(
                        "accounts"
                            | "create_managed_account"
                            | "revoke_account"
                            | "rename_account"
                            | "set_default_account"
                            | "export_managed_secret"
                            | "import_managed_secret"
                    )
                ),
                "account operation emitted a retired raw Wallet request: {request}"
            );
            if operation != Some(WALLET_BUS_OPERATION) {
                continue;
            }
            assert_eq!(request["_runtime_invocation"]["source"], "runtime");
            assert_eq!(request["_runtime_invocation"]["target"], "wallet");
            assert_eq!(request["_runtime_invocation"]["op"], WALLET_BUS_OPERATION);
            assert_eq!(
                request["_runtime_invocation"]["transport"],
                "runtime-local-provider-plane"
            );
            assert_eq!(
                request["_runtime_invocation"]["carrier"],
                serde_json::Value::Null
            );
            let request_bytes = serde_json::to_vec(
                request
                    .get("request")
                    .expect("Wallet Bus v2 request envelope"),
            )
            .unwrap();
            let wallet_request =
                WalletProviderRequestV2::decode_at(&request_bytes, crate::auth::now_ts()).unwrap();
            let kind = wallet_request.operation.kind();
            if matches!(
                kind,
                WalletOperationKind::ExportManagedRecoveryKey
                    | WalletOperationKind::ImportManagedRecoveryKey
            ) {
                assert!(
                    request["request"]["operation"]["params"]
                        .get("principal_id")
                        .is_none(),
                    "managed Recovery Key operation supplied principal_id outside Runtime authority"
                );
            }
            if !matches!(
                kind,
                WalletOperationKind::ListAccounts
                    | WalletOperationKind::CreateManagedAccount
                    | WalletOperationKind::RevokeAccount
                    | WalletOperationKind::RenameAccount
                    | WalletOperationKind::SetDefaultAccount
                    | WalletOperationKind::ExportManagedRecoveryKey
                    | WalletOperationKind::ImportManagedRecoveryKey
            ) {
                continue;
            }
            if let WalletProviderOperationV2::ListAccounts { include_revoked } =
                &wallet_request.operation
            {
                assert!(!include_revoked);
            }
            assert_eq!(wallet_request.authority.actor, expected.actor());
            assert_eq!(
                wallet_request.authority.principal_id,
                expected.principal_id()
            );
            assert_eq!(wallet_request.authority.session_id, expected.session_id());
            assert_eq!(
                wallet_request.authority.proof_binding_id.as_deref(),
                expected.proof_binding_id()
            );
            assert_eq!(wallet_request.authority.grant_id, expected.grant_id());
            assert_eq!(wallet_request.authority.launch_id, expected.launch_id());
            actual.push(kind);
        }
        assert_eq!(actual, expected_operations);
    }

    async fn assert_v2_approval_operations(
        &self,
        expected_authority: &RuntimeWalletAuthority,
        expected_operations: &[WalletOperationKind],
    ) {
        let expected = expected_authority.verified_context();
        let requests = self.requests.lock().await;
        let mut actual = Vec::new();
        for request in requests.iter() {
            let operation = request.get("op").and_then(Value::as_str);
            assert!(
                !matches!(
                    operation,
                    Some(
                        "approval_requests"
                            | "request_signature"
                            | "reject_approval"
                            | "approve_approval"
                            | "sign_approved"
                            | "complete_approval"
                    )
                ),
                "approval operation emitted a retired raw Wallet request: {request}"
            );
            if operation != Some(WALLET_BUS_OPERATION) {
                continue;
            }
            let request_bytes = serde_json::to_vec(
                request
                    .get("request")
                    .expect("Wallet Bus v2 request envelope"),
            )
            .unwrap();
            let wallet_request =
                WalletProviderRequestV2::decode_at(&request_bytes, crate::auth::now_ts()).unwrap();
            let kind = wallet_request.operation.kind();
            if !matches!(
                kind,
                WalletOperationKind::RequestApproval
                    | WalletOperationKind::ListApprovals
                    | WalletOperationKind::RejectApproval
                    | WalletOperationKind::ApproveAndSignManaged
                    | WalletOperationKind::ApproveConnectorHandoff
                    | WalletOperationKind::CompleteConnectorHandoff
                    | WalletOperationKind::AttachValidatedChainOutcome
            ) {
                continue;
            }
            if wallet_request.authority.actor != expected.actor() {
                continue;
            }
            assert_eq!(
                wallet_request.authority.principal_id,
                expected.principal_id()
            );
            assert_eq!(wallet_request.authority.session_id, expected.session_id());
            assert_eq!(
                wallet_request.authority.proof_binding_id.as_deref(),
                expected.proof_binding_id()
            );
            assert_eq!(wallet_request.authority.grant_id, expected.grant_id());
            assert_eq!(wallet_request.authority.launch_id, expected.launch_id());
            actual.push(kind);
        }
        assert_eq!(actual, expected_operations);
    }
}

impl MockWalletProvider {
    async fn seed_managed_evm_account_for_principal(&self, principal_id: &str) -> String {
        let account_id = format!("wallet:eip155:8453:{MOCK_MANAGED_EVM_ADDRESS}");
        let account = json!({
            "account_id": account_id,
            "principal_id": principal_id,
            "proof_binding_id": format!(
                "proof:wallet:managed:eip155:8453:{MOCK_MANAGED_EVM_ADDRESS}"
            ),
            "chain_namespace": "eip155:8453",
            "address": MOCK_MANAGED_EVM_ADDRESS,
            "proof_type": "managed_evm",
            "signing_available": true,
            "signing_status": "managed_key_available",
            "label": "Managed",
            "linked_at": crate::auth::now_ts()
        });
        let mut accounts = self.accounts.lock().await;
        if let Some(existing) = accounts.iter_mut().find(|existing| {
            existing.get("principal_id").and_then(Value::as_str) == Some(principal_id)
                && existing.get("account_id").and_then(Value::as_str) == Some(account_id.as_str())
        }) {
            *existing = account;
        } else {
            accounts.push(account);
        }
        account_id
    }

    async fn latest_transaction_approval_request_id(&self) -> Option<String> {
        let approvals = self.approvals.lock().await;
        approvals
            .iter()
            .rev()
            .find(|approval| {
                approval.get("intent").and_then(Value::as_str) == Some("transaction_intent")
            })
            .and_then(|approval| approval.get("request_id").and_then(Value::as_str))
            .map(ToOwned::to_owned)
    }

    async fn latest_transaction_signed_transaction(&self) -> Option<String> {
        let approvals = self.approvals.lock().await;
        approvals
            .iter()
            .rev()
            .find(|approval| {
                approval.get("intent").and_then(Value::as_str) == Some("transaction_intent")
            })
            .and_then(|approval| approval.get("signed_result"))
            .and_then(|result| result.get("signed_transaction"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
    }

    async fn complete_latest_transaction_approval(&self) -> String {
        let mut approvals = self.approvals.lock().await;
        let approval = approvals
            .iter_mut()
            .rev()
            .find(|approval| {
                approval.get("intent").and_then(Value::as_str) == Some("transaction_intent")
            })
            .expect("mock transaction approval");
        let signed_transaction = mock_sign_eip155_transaction(
            approval
                .get("payload")
                .expect("mock transaction approval payload"),
        )
        .unwrap();
        let transaction_hash = signed_evm_transaction_hash_for_test(&signed_transaction);
        approval["status"] = serde_json::Value::String("completed".to_string());
        approval["signed_result"] = json!({
            "schema": "elastos.wallet.signed-transaction-result/v1",
            "request_id": approval.get("request_id").cloned().unwrap_or(json!("wallet-request:test")),
            "method": "eth_sendTransaction",
            "signed_transaction": signed_transaction,
            "transaction_hash": transaction_hash,
            "signer": approval.get("address").cloned().unwrap_or(json!(MOCK_MANAGED_EVM_ADDRESS)),
            "chain_namespace": approval.get("chain_namespace").cloned().unwrap_or(json!("eip155:8453")),
            "payload_hash": approval.get("payload_hash").cloned().unwrap_or(json!("0x00")),
        });
        transaction_hash
    }

    async fn send_legacy_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        match request.get("op").and_then(|value| value.as_str()) {
            Some("challenge") => {
                let domain = required_test_str(request, "domain")?;
                let uri = required_test_str(request, "uri")?;
                let address = required_test_str(request, "address")?;
                let chain_id = request
                    .get("chain_id")
                    .and_then(|value| value.as_u64())
                    .ok_or_else(|| ProviderError::Provider("missing chain_id".into()))?;
                let mut resources = vec![String::new()];
                resources.extend(required_test_string_array(request, "resources")?);
                let mut challenges = self.challenges.lock().await;
                let challenge_id = format!("wallet-test-{}", challenges.len() + 1);
                resources[0] = format!("elastos://auth/challenge/{challenge_id}");
                let challenge = AuthChallengeV1::new(AuthChallengeInput {
                    challenge_id: challenge_id.clone(),
                    domain: domain.to_string(),
                    uri: uri.to_string(),
                    address: address.to_string(),
                    chain_id,
                    nonce: format!("nonce{:08}", challenges.len() + 1),
                    issued_at: crate::auth::now_ts(),
                    ttl_secs: 300,
                    resources,
                });
                challenges.insert(
                    challenge_id.clone(),
                    MockWalletChallenge {
                        challenge: challenge.clone(),
                        consumed: false,
                    },
                );
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "schema": AuthChallengeV1::SCHEMA,
                        "challenge_id": challenge_id,
                        "message": challenge.siwe_message(),
                        "expires_at": challenge.expires_at,
                        "resources": challenge.resources,
                    }
                }))
            }
            Some("bitcoin_challenge") => {
                let domain = required_test_str(request, "domain")?;
                let uri = required_test_str(request, "uri")?;
                let address = required_test_str(request, "address")?;
                let network = required_test_str(request, "network")?;
                let mut resources = vec![String::new()];
                resources.extend(required_test_string_array(request, "resources")?);
                let mut challenges = self.bitcoin_challenges.lock().await;
                let challenge_id = format!("bitcoin-test-{}", challenges.len() + 1);
                resources[0] = format!("elastos://auth/bitcoin-challenge/{challenge_id}");
                let now = crate::auth::now_ts();
                let message = format!(
                    "{domain} wants you to prove Bitcoin account ownership:\n{address}\n\nURI: {uri}\nVersion: 1\nNetwork: {network}\nNonce: bitcoin-nonce\nIssued At: {now}\nExpiration Time: {expires_at}\nResources:\n{resources}",
                    expires_at = now + 300,
                    resources = resources
                        .iter()
                        .map(|resource| format!("- {resource}"))
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
                challenges.insert(
                    challenge_id.clone(),
                    MockBitcoinChallenge {
                        message: message.clone(),
                        address: address.to_string(),
                        consumed: false,
                    },
                );
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "schema": "elastos.wallet.bitcoin_challenge/v1",
                        "challenge_id": challenge_id,
                        "message": message,
                        "expires_at": now + 300,
                        "network": network,
                        "address": address,
                        "resources": resources,
                        "proof_type": "bip322_simple",
                    }
                }))
            }
            Some("verify_proof") => {
                let message = required_test_str(request, "message")?;
                let signature = required_test_str(request, "signature")?;
                let parsed = elastos_runtime::auth::parse_siwe_message(message)
                    .map_err(ProviderError::Provider)?;
                let challenge_id = parsed
                    .resources
                    .iter()
                    .find_map(|resource| resource.strip_prefix("elastos://auth/challenge/"))
                    .ok_or_else(|| ProviderError::Provider("missing challenge resource".into()))?;
                let mut challenges = self.challenges.lock().await;
                let stored = challenges
                    .get_mut(challenge_id)
                    .ok_or_else(|| ProviderError::Provider("challenge not found".into()))?;
                if stored.consumed {
                    return Ok(json!({
                        "status": "error",
                        "code": "invalid_proof",
                        "message": "challenge already consumed"
                    }));
                }
                let proof = match verify_siwe_challenge(
                    &stored.challenge,
                    message,
                    signature,
                    crate::auth::now_ts(),
                ) {
                    Ok(proof) => proof,
                    Err(err) => {
                        return Ok(json!({
                            "status": "error",
                            "code": "invalid_proof",
                            "message": err
                        }));
                    }
                };
                stored.consumed = true;
                let proof_binding_id = proof.binding.id();
                let chain_id = proof.binding.chain_id.unwrap_or_default();
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "schema": "elastos.wallet.proof/v1",
                        "proof_binding_id": proof_binding_id,
                        "chain_namespace": format!("eip155:{chain_id}"),
                        "address": proof.recovered_address,
                        "proof_type": "siwe",
                        "challenge_id": challenge_id,
                        "verified_at": crate::auth::now_ts(),
                        "message_hash": format!("0x{}", hex::encode(proof.message_hash)),
                    }
                }))
            }
            Some("verify_bip322_proof") => {
                let message = required_test_str(request, "message")?;
                let signature = required_test_str(request, "signature")?;
                let challenge_id = message
                    .lines()
                    .find_map(|line| {
                        line.trim()
                            .strip_prefix("- elastos://auth/bitcoin-challenge/")
                    })
                    .ok_or_else(|| ProviderError::Provider("missing Bitcoin challenge".into()))?;
                let mut challenges = self.bitcoin_challenges.lock().await;
                let stored = challenges
                    .get_mut(challenge_id)
                    .ok_or_else(|| ProviderError::Provider("Bitcoin challenge not found".into()))?;
                if stored.consumed {
                    return Ok(json!({
                        "status": "error",
                        "code": "invalid_proof",
                        "message": "Bitcoin challenge already consumed"
                    }));
                }
                if message != stored.message {
                    return Ok(json!({
                        "status": "error",
                        "code": "invalid_proof",
                        "message": "Bitcoin challenge message does not match"
                    }));
                }
                if signature != "mock-bip322-signature" {
                    return Ok(json!({
                        "status": "error",
                        "code": "invalid_bip322_proof",
                        "message": "invalid mock BIP-322 signature"
                    }));
                }
                stored.consumed = true;
                let chain_namespace = "bip122:000000000019d6689c085ae165831e93";
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "schema": "elastos.wallet.proof/v1",
                        "proof_binding_id": format!("proof:wallet:{chain_namespace}:{}", stored.address),
                        "chain_namespace": chain_namespace,
                        "address": stored.address,
                        "proof_type": "bip322_simple",
                        "proof_strength": "standard",
                        "challenge_id": challenge_id,
                        "verified_at": crate::auth::now_ts(),
                        "message_hash": "0x010203",
                    }
                }))
            }
            Some("verify_contract_proof") => {
                let message = required_test_str(request, "message")?;
                let signature = required_test_str(request, "signature")?;
                let proof = request
                    .get("erc1271_proof")
                    .ok_or_else(|| ProviderError::Provider("missing erc1271_proof".into()))?;
                let parsed = elastos_runtime::auth::parse_siwe_message(message)
                    .map_err(ProviderError::Provider)?;
                let challenge_id = parsed
                    .resources
                    .iter()
                    .find_map(|resource| resource.strip_prefix("elastos://auth/challenge/"))
                    .ok_or_else(|| ProviderError::Provider("missing challenge resource".into()))?;
                let mut challenges = self.challenges.lock().await;
                let stored = challenges
                    .get_mut(challenge_id)
                    .ok_or_else(|| ProviderError::Provider("challenge not found".into()))?;
                if stored.consumed {
                    return Ok(json!({
                        "status": "error",
                        "code": "invalid_proof",
                        "message": "challenge already consumed"
                    }));
                }
                if message != stored.challenge.siwe_message() {
                    return Ok(json!({
                        "status": "error",
                        "code": "invalid_proof",
                        "message": "SIWE message does not match challenge"
                    }));
                }
                let message_hash = ethereum_signed_message_hash(message.as_bytes());
                let expected_message_hash = format!("0x{}", hex::encode(message_hash));
                let signature_bytes = hex::decode(signature.trim_start_matches("0x"))
                    .map_err(|err| ProviderError::Provider(err.to_string()))?;
                let expected_signature_hash =
                    format!("0x{}", hex::encode(sha2::Sha256::digest(signature_bytes)));
                if proof.get("valid").and_then(|value| value.as_bool()) != Some(true)
                    || proof.get("chain_id").and_then(|value| value.as_u64())
                        != Some(parsed.chain_id)
                    || proof.get("contract").and_then(|value| value.as_str())
                        != Some(parsed.address.as_str())
                    || proof.get("message_hash").and_then(|value| value.as_str())
                        != Some(expected_message_hash.as_str())
                    || proof.get("signature_hash").and_then(|value| value.as_str())
                        != Some(expected_signature_hash.as_str())
                {
                    return Ok(json!({
                        "status": "error",
                        "code": "invalid_contract_proof",
                        "message": "ERC-1271 proof mismatch"
                    }));
                }
                stored.consumed = true;
                let proof_binding_id = ProofBinding::evm_account(
                    parsed.chain_id,
                    &parsed.address,
                    crate::auth::now_ts(),
                )
                .id();
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "schema": "elastos.wallet.proof/v1",
                        "proof_binding_id": proof_binding_id,
                        "chain_namespace": format!("eip155:{}", parsed.chain_id),
                        "address": parsed.address,
                        "proof_type": "siwe_erc1271",
                        "challenge_id": challenge_id,
                        "verified_at": crate::auth::now_ts(),
                        "message_hash": expected_message_hash,
                    }
                }))
            }
            Some("link_account") => {
                let chain_namespace = required_test_str(request, "chain_namespace")?;
                let address = required_test_str(request, "address")?;
                let connector_id = required_test_str(request, "connector_id")?;
                let account = json!({
                    "account_id": format!("wallet:{chain_namespace}:{address}"),
                    "principal_id": required_test_str(request, "principal_id")?,
                    "proof_binding_id": required_test_str(request, "proof_binding_id")?,
                    "chain_namespace": chain_namespace,
                    "address": address,
                    "proof_type": required_test_str(request, "proof_type")?,
                    "connector_id": connector_id,
                    "linked_at": crate::auth::now_ts()
                });
                let mut accounts = self.accounts.lock().await;
                accounts.push(account.clone());
                Ok(json!({
                    "status": "ok",
                    "data": { "account": account }
                }))
            }
            Some("accounts") => {
                let principal_id = required_test_str(request, "principal_id")?;
                let accounts = self.accounts.lock().await;
                let visible = accounts
                    .iter()
                    .filter(|account| {
                        account.get("principal_id").and_then(|value| value.as_str())
                            == Some(principal_id)
                            && account.get("revoked_at").is_none()
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                let defaults = self.defaults.lock().await;
                let visible_defaults = defaults
                    .iter()
                    .filter(|default| {
                        default.get("principal_id").and_then(|value| value.as_str())
                            == Some(principal_id)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "accounts": visible,
                        "default_accounts": visible_defaults
                    }
                }))
            }
            Some("set_default_account") => {
                let principal_id = required_test_str(request, "principal_id")?;
                let chain_namespace = required_test_str(request, "chain_namespace")?;
                let intent = required_test_str(request, "intent")?;
                let account_id = required_test_str(request, "account_id")?;
                let accounts = self.accounts.lock().await;
                let Some(account) = accounts.iter().find(|account| {
                    account.get("principal_id").and_then(|value| value.as_str())
                        == Some(principal_id)
                        && account.get("account_id").and_then(|value| value.as_str())
                            == Some(account_id)
                }) else {
                    return Ok(json!({
                        "status": "error",
                        "code": "not_found",
                        "message": "active linked account not found"
                    }));
                };
                if account
                    .get("chain_namespace")
                    .and_then(|value| value.as_str())
                    != Some(chain_namespace)
                {
                    return Ok(json!({
                        "status": "error",
                        "code": "invalid_request",
                        "message": "default wallet chain must match the linked account"
                    }));
                }
                drop(accounts);
                let default_account = json!({
                    "schema": "elastos.wallet.default_account/v1",
                    "principal_id": principal_id,
                    "chain_namespace": chain_namespace,
                    "intent": intent,
                    "account_id": account_id,
                    "set_at": crate::auth::now_ts()
                });
                let mut defaults = self.defaults.lock().await;
                if let Some(existing) = defaults.iter_mut().find(|existing| {
                    existing
                        .get("principal_id")
                        .and_then(|value| value.as_str())
                        == Some(principal_id)
                        && existing
                            .get("chain_namespace")
                            .and_then(|value| value.as_str())
                            == Some(chain_namespace)
                        && existing.get("intent").and_then(|value| value.as_str()) == Some(intent)
                }) {
                    *existing = default_account.clone();
                } else {
                    defaults.push(default_account.clone());
                }
                Ok(json!({
                    "status": "ok",
                    "data": { "default_account": default_account }
                }))
            }
            Some("create_managed_account") => {
                let principal_id = required_test_str(request, "principal_id")?;
                let chain_namespace = required_test_str(request, "chain_namespace")?;
                let create_new = request
                    .get("create_new")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false);
                let (_base_address, proof_type) =
                    if chain_namespace == "bip122:000000000019d6689c085ae165831e93" {
                        (
                            "bc1q9vza2e8x573nczrlzms0wvx3gsqjx7vavgkx0l",
                            "managed_btc_p2wpkh",
                        )
                    } else {
                        ("0x1111111111111111111111111111111111111111", "managed_evm")
                    };
                let mut accounts = self.accounts.lock().await;
                if !create_new {
                    if let Some(account) = accounts.iter().find(|account| {
                        account.get("principal_id").and_then(|value| value.as_str())
                            == Some(principal_id)
                            && account
                                .get("chain_namespace")
                                .and_then(|value| value.as_str())
                                == Some(chain_namespace)
                            && account.get("proof_type").and_then(|value| value.as_str())
                                == Some(proof_type)
                    }) {
                        return Ok(json!({
                            "status": "ok",
                            "data": { "account": account, "created": false }
                        }));
                    }
                }
                let address = if chain_namespace == "bip122:000000000019d6689c085ae165831e93" {
                    if create_new {
                        format!(
                            "bc1q9vza2e8x573nczrlzms0wvx3gsqjx7vavgkx{:02}",
                            accounts.len()
                        )
                    } else {
                        "bc1q9vza2e8x573nczrlzms0wvx3gsqjx7vavgkx0l".to_string()
                    }
                } else if create_new {
                    mock_managed_evm_address(accounts.len() + 1)?
                } else {
                    "0x1111111111111111111111111111111111111111".to_string()
                };
                let account = json!({
                    "account_id": format!("wallet:{chain_namespace}:{address}"),
                    "principal_id": principal_id,
                    "proof_binding_id": format!("proof:wallet:managed:{chain_namespace}:{address}"),
                    "chain_namespace": chain_namespace,
                    "address": address,
                    "proof_type": proof_type,
                    "signing_available": true,
                    "signing_status": "managed_key_available",
                    "label": request.get("label").and_then(|value| value.as_str()).unwrap_or("Managed"),
                    "linked_at": crate::auth::now_ts()
                });
                accounts.push(account.clone());
                Ok(json!({
                    "status": "ok",
                    "data": { "account": account, "created": true }
                }))
            }
            Some("revoke_account") => {
                let principal_id = required_test_str(request, "principal_id")?;
                let account_id = required_test_str(request, "account_id")?;
                let mut accounts = self.accounts.lock().await;
                let Some(account) = accounts.iter_mut().find(|account| {
                    account.get("principal_id").and_then(|value| value.as_str())
                        == Some(principal_id)
                        && account.get("account_id").and_then(|value| value.as_str())
                            == Some(account_id)
                }) else {
                    return Ok(json!({
                        "status": "error",
                        "code": "not_found",
                        "message": "linked account not found"
                    }));
                };
                account["revoked_at"] = json!(crate::auth::now_ts());
                Ok(json!({
                    "status": "ok",
                    "data": { "account": account.clone() }
                }))
            }
            Some("rename_account") => {
                let principal_id = required_test_str(request, "principal_id")?;
                let account_id = required_test_str(request, "account_id")?;
                let label = required_test_str(request, "label")?.trim().to_string();
                if label.is_empty() {
                    return Ok(json!({
                        "status": "error",
                        "code": "invalid_request",
                        "message": "label is required"
                    }));
                }
                let mut accounts = self.accounts.lock().await;
                let Some(account) = accounts.iter_mut().find(|account| {
                    account.get("principal_id").and_then(|value| value.as_str())
                        == Some(principal_id)
                        && account.get("account_id").and_then(|value| value.as_str())
                            == Some(account_id)
                        && account.get("revoked_at").is_none()
                }) else {
                    return Ok(json!({
                        "status": "error",
                        "code": "not_found",
                        "message": "active linked account not found"
                    }));
                };
                account["label"] = json!(label);
                Ok(json!({
                    "status": "ok",
                    "data": { "account": account.clone() }
                }))
            }
            Some("export_managed_recovery_set") => {
                let principal_id = required_test_str(request, "principal_id")?;
                let accounts = self.accounts.lock().await;
                let keys = accounts
                    .iter()
                    .filter(|account| {
                        account.get("principal_id").and_then(Value::as_str) == Some(principal_id)
                            && account.get("revoked_at").is_none()
                            && account.get("connector_id").and_then(Value::as_str).is_none()
                            && account
                                .get("proof_type")
                                .and_then(Value::as_str)
                                .is_some_and(|proof| {
                                    proof == "managed_evm" || proof == "managed_btc_p2wpkh"
                                })
                    })
                    .map(|account| {
                        json!({
                            "account_id": account["account_id"],
                            "recovery_key": {
                                "schema": "elastos.wallet.recovery-key/v1",
                                "account_id": account["account_id"],
                                "chain_namespace": account["chain_namespace"],
                                "address": account["address"],
                                "secret_type": "secp256k1_private_key_hex",
                                "private_key_hex": "1111111111111111111111111111111111111111111111111111111111111111",
                                "note": "This account was created as an encrypted signing key, not a BIP39 seed phrase."
                            },
                            "label": account.get("label").cloned().unwrap_or(Value::Null),
                        })
                    })
                    .collect::<Vec<_>>();
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "schema": "elastos.wallet.managed-recovery-set/v1",
                        "keys": keys,
                    }
                }))
            }
            Some("import_managed_recovery_set") => {
                let principal_id = required_test_str(request, "principal_id")?;
                let recovery_set = request
                    .get("recovery_set")
                    .ok_or_else(|| ProviderError::Provider("missing recovery_set".into()))?;
                let entries = recovery_set
                    .get("keys")
                    .and_then(Value::as_array)
                    .ok_or_else(|| ProviderError::Provider("missing recovery set keys".into()))?;
                let mut imported_accounts = Vec::with_capacity(entries.len());
                for entry in entries {
                    let account_id = required_test_str(entry, "account_id")?;
                    let recovery_key = entry
                        .get("recovery_key")
                        .ok_or_else(|| ProviderError::Provider("missing recovery_key".into()))?;
                    if required_test_str(recovery_key, "account_id")? != account_id {
                        return Ok(json!({
                            "status": "error",
                            "code": "invalid_request",
                            "message": "managed recovery set account_id mismatch"
                        }));
                    }
                    let chain_namespace = required_test_str(recovery_key, "chain_namespace")?;
                    let address = required_test_str(recovery_key, "address")?;
                    let proof_type = if chain_namespace == "bip122:000000000019d6689c085ae165831e93"
                    {
                        "managed_btc_p2wpkh"
                    } else {
                        "managed_evm"
                    };
                    imported_accounts.push(json!({
                        "account_id": account_id,
                        "principal_id": principal_id,
                        "proof_binding_id": format!("proof:wallet:managed:{chain_namespace}:{address}"),
                        "chain_namespace": chain_namespace,
                        "address": address,
                        "proof_type": proof_type,
                        "signing_available": true,
                        "signing_status": "managed_key_available",
                        "label": entry.get("label").cloned().unwrap_or_else(|| json!("Imported")),
                        "linked_at": crate::auth::now_ts()
                    }));
                }
                let mut accounts = self.accounts.lock().await;
                for imported in &imported_accounts {
                    if let Some(existing) = accounts.iter_mut().find(|account| {
                        account.get("principal_id") == imported.get("principal_id")
                            && account.get("account_id") == imported.get("account_id")
                    }) {
                        *existing = imported.clone();
                    } else {
                        accounts.push(imported.clone());
                    }
                }
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "imported": true,
                        "account_count": imported_accounts.len(),
                        "accounts": imported_accounts,
                    }
                }))
            }
            Some("export_managed_secret") => {
                let principal_id = required_test_str(request, "principal_id")?;
                let account_id = required_test_str(request, "account_id")?;
                let accounts = self.accounts.lock().await;
                let Some(account) = accounts.iter().find(|account| {
                    account.get("principal_id").and_then(|value| value.as_str())
                        == Some(principal_id)
                        && account.get("account_id").and_then(|value| value.as_str())
                            == Some(account_id)
                        && account.get("revoked_at").is_none()
                }) else {
                    return Ok(json!({
                        "status": "error",
                        "code": "not_found",
                        "message": "active linked account not found"
                    }));
                };
                if !account
                    .get("proof_type")
                    .and_then(|value| value.as_str())
                    .is_some_and(|proof| proof == "managed_evm" || proof == "managed_btc_p2wpkh")
                {
                    return Ok(json!({
                        "status": "error",
                        "code": "external_wallet_required",
                        "message": "recovery key is available only for passkey-managed accounts"
                    }));
                }
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "schema": "elastos.wallet.recovery-key/v1",
                        "account_id": account_id,
                        "chain_namespace": account["chain_namespace"],
                        "address": account["address"],
                        "secret_type": "secp256k1_private_key_hex",
                        "private_key_hex": "1111111111111111111111111111111111111111111111111111111111111111",
                        "note": "This account was created as an encrypted signing key, not a BIP39 seed phrase."
                    }
                }))
            }
            Some("import_managed_secret") => {
                let principal_id = required_test_str(request, "principal_id")?;
                let Some(recovery_key) = request.get("recovery_key") else {
                    return Ok(json!({
                        "status": "error",
                        "code": "invalid_request",
                        "message": "recovery_key is required"
                    }));
                };
                if recovery_key.get("schema").and_then(|value| value.as_str())
                    != Some("elastos.wallet.recovery-key/v1")
                {
                    return Ok(json!({
                        "status": "error",
                        "code": "invalid_request",
                        "message": "expected elastos.wallet.recovery-key/v1"
                    }));
                }
                let account_id = required_test_str(recovery_key, "account_id")?;
                let chain_namespace = required_test_str(recovery_key, "chain_namespace")?;
                let address = required_test_str(recovery_key, "address")?;
                let proof_type = if chain_namespace == "bip122:000000000019d6689c085ae165831e93" {
                    "managed_btc_p2wpkh"
                } else {
                    "managed_evm"
                };
                let label = request
                    .get("label")
                    .and_then(|value| value.as_str())
                    .unwrap_or("Imported");
                let imported = json!({
                    "account_id": account_id,
                    "principal_id": principal_id,
                    "proof_binding_id": format!("proof:wallet:managed:{chain_namespace}:{address}"),
                    "chain_namespace": chain_namespace,
                    "address": address,
                    "proof_type": proof_type,
                    "signing_available": true,
                    "signing_status": "managed_key_available",
                    "label": label,
                    "linked_at": crate::auth::now_ts()
                });
                let mut accounts = self.accounts.lock().await;
                if let Some(existing) = accounts.iter_mut().find(|account| {
                    account.get("principal_id").and_then(|value| value.as_str())
                        == Some(principal_id)
                        && account.get("account_id").and_then(|value| value.as_str())
                            == Some(account_id)
                }) {
                    *existing = imported.clone();
                } else {
                    accounts.push(imported.clone());
                }
                Ok(json!({
                    "status": "ok",
                    "data": { "account": imported, "imported": true }
                }))
            }
            Some("approval_requests") => {
                let principal_id = required_test_str(request, "principal_id")?;
                let include_resolved = request
                    .get("include_resolved")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false);
                let approvals = self.approvals.lock().await;
                let approval_requests = approvals
                    .iter()
                    .filter(|approval| {
                        approval
                            .get("principal_id")
                            .and_then(|value| value.as_str())
                            == Some(principal_id)
                    })
                    .filter(|approval| {
                        include_resolved
                            || approval.get("status").and_then(|value| value.as_str())
                                == Some("pending")
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                Ok(json!({
                    "status": "ok",
                    "data": { "approval_requests": approval_requests }
                }))
            }
            Some("request_signature") => {
                let principal_id = required_test_str(request, "principal_id")?;
                let chain_namespace = required_test_str(request, "chain_namespace")?;
                let intent = required_test_str(request, "intent")?;
                let accounts = self.accounts.lock().await;
                let account_id = match request.get("account_id").and_then(|value| value.as_str()) {
                    Some(account_id) => account_id.to_string(),
                    None => {
                        let defaults = self.defaults.lock().await;
                        let Some(default) = defaults.iter().find(|default| {
                            default.get("principal_id").and_then(|value| value.as_str())
                                == Some(principal_id)
                                && default
                                    .get("chain_namespace")
                                    .and_then(|value| value.as_str())
                                    == Some(chain_namespace)
                                && default.get("intent").and_then(|value| value.as_str())
                                    == Some(intent)
                        }) else {
                            return Ok(json!({
                                "status": "error",
                                "code": "not_found",
                                "message": "default linked account not set"
                            }));
                        };
                        default
                            .get("account_id")
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                            .to_string()
                    }
                };
                let Some(account) = accounts.iter().find(|account| {
                    account.get("principal_id").and_then(|value| value.as_str())
                        == Some(principal_id)
                        && account.get("account_id").and_then(|value| value.as_str())
                            == Some(account_id.as_str())
                        && account
                            .get("chain_namespace")
                            .and_then(|value| value.as_str())
                            == Some(chain_namespace)
                }) else {
                    return Ok(json!({
                        "status": "error",
                        "code": "not_found",
                        "message": "active linked account not found"
                    }));
                };
                let account = account.clone();
                let payload = request.get("payload").cloned().unwrap_or_else(|| json!({}));
                let payload_bytes = serde_json::to_vec(&payload)
                    .map_err(|err| ProviderError::Provider(err.to_string()))?;
                let payload_hash = format!("0x{}", hex::encode(Keccak256::digest(&payload_bytes)));
                drop(accounts);
                let mut approvals = self.approvals.lock().await;
                let request_id = request
                    .get("request_id")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
                    .unwrap_or_else(|| format!("wallet-approval:mock-{}", approvals.len() + 1));
                if let Some(existing) = approvals.iter().find(|approval| {
                    approval.get("request_id").and_then(Value::as_str) == Some(request_id.as_str())
                }) {
                    return Ok(json!({
                        "status": "ok",
                        "data": {
                            "approval_request": existing,
                            "requires_approval": existing.get("status").and_then(Value::as_str) == Some("pending"),
                            "signature": serde_json::Value::Null
                        }
                    }));
                }
                let approval = json!({
                    "schema": "elastos.wallet.approval_request/v1",
                    "request_id": request_id,
                    "wallet_request_sha256": request.get("wallet_request_sha256").cloned().unwrap_or(json!("legacy")),
                    "authority_binding": request.get("authority_binding").cloned().unwrap_or(json!("legacy")),
                    "kind": "signature",
                    "status": "pending",
                    "intent": intent,
                    "capsule_id": required_test_str(request, "capsule_id")?,
                    "requested_by_actor": required_test_str(request, "capsule_id")?,
                    "resource": required_test_str(request, "resource")?,
                    "reason": required_test_str(request, "reason")?,
                    "account_id": account_id,
                    "chain_namespace": chain_namespace,
                    "address": account.get("address").cloned().unwrap_or(json!("0x0")),
                    "proof_binding_id": account.get("proof_binding_id").cloned().unwrap_or(json!("proof:wallet:test")),
                    "proof_type": account.get("proof_type").cloned().unwrap_or(json!("siwe")),
                    "connector_id": account.get("connector_id").cloned().unwrap_or(json!(null)),
                    "payload_hash": payload_hash,
                    "payload": payload,
                    "principal_id": principal_id,
                    "session_id": request.get("session_id").cloned().unwrap_or(json!("session:test")),
                    "launch_id": request.get("launch_id").cloned().unwrap_or(json!("launch:test")),
                    "created_at": crate::auth::now_ts(),
                    "expires_at": request
                        .get("expires_at")
                        .and_then(Value::as_u64)
                        .unwrap_or_else(|| crate::auth::now_ts() + 600)
                });
                approvals.push(approval.clone());
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "approval_request": approval,
                        "requires_approval": true,
                        "signature": serde_json::Value::Null
                    }
                }))
            }
            Some("reject_approval") => {
                let principal_id = required_test_str(request, "principal_id")?;
                let request_id = required_test_str(request, "request_id")?;
                let mut approvals = self.approvals.lock().await;
                let Some(approval) = approvals.iter_mut().find(|approval| {
                    approval
                        .get("principal_id")
                        .and_then(|value| value.as_str())
                        == Some(principal_id)
                        && approval.get("request_id").and_then(|value| value.as_str())
                            == Some(request_id)
                }) else {
                    return Ok(json!({
                        "status": "error",
                        "code": "not_found",
                        "message": "wallet approval request not found"
                    }));
                };
                approval["status"] = serde_json::Value::String("rejected".to_string());
                Ok(json!({
                    "status": "ok",
                    "data": { "approval_request": approval.clone() }
                }))
            }
            Some("approve_and_sign_managed") => {
                let principal_id = required_test_str(request, "principal_id")?;
                let request_id = required_test_str(request, "request_id")?;
                let mut approvals = self.approvals.lock().await;
                let Some(approval) = approvals.iter_mut().find(|approval| {
                    approval
                        .get("principal_id")
                        .and_then(|value| value.as_str())
                        == Some(principal_id)
                        && approval.get("request_id").and_then(|value| value.as_str())
                            == Some(request_id)
                }) else {
                    return Ok(json!({
                        "status": "error",
                        "code": "not_found",
                        "message": "wallet approval request not found"
                    }));
                };
                if approval.get("status").and_then(|value| value.as_str()) != Some("pending") {
                    return Ok(json!({
                        "status": "error",
                        "code": "invalid_request",
                        "message": "wallet approval request is not pending"
                    }));
                }
                if !approval
                    .get("proof_type")
                    .and_then(Value::as_str)
                    .is_some_and(is_managed_wallet_proof_type)
                {
                    return Ok(json!({
                        "status": "error",
                        "code": "external_wallet_required",
                        "message": "connector approvals require a typed connector handoff"
                    }));
                }
                approval["status"] = json!("completed");
                approval["signature_receipt"] = json!({
                    "schema": "elastos.wallet.signature_receipt/v1",
                    "request_id": request_id,
                    "signer": approval.get("address").cloned().unwrap_or(json!("0x0")),
                    "payload_hash": approval.get("payload_hash").cloned().unwrap_or(json!("0x0000000000000000000000000000000000000000000000000000000000000000")),
                    "signature_hash": "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "completed_at": crate::auth::now_ts(),
                });
                if approval.get("intent").and_then(|value| value.as_str())
                    == Some("transaction_intent")
                {
                    let signed_transaction = mock_sign_eip155_transaction(
                        approval.get("payload").ok_or_else(|| {
                            ProviderError::Provider(
                                "mock transaction approval is missing payload".to_string(),
                            )
                        })?,
                    )?;
                    let transaction_hash =
                        signed_evm_transaction_hash_for_test(&signed_transaction);
                    approval["signed_result"] = json!({
                        "schema": "elastos.wallet.signed-transaction-result/v1",
                        "request_id": request_id,
                        "method": "eth_sendTransaction",
                        "signed_transaction": signed_transaction,
                        "transaction_hash": transaction_hash,
                        "signer": approval.get("address").cloned().unwrap_or(json!("0x0")),
                        "chain_namespace": approval.get("chain_namespace").cloned().unwrap_or(json!("eip155:20")),
                        "payload_hash": approval.get("payload_hash").cloned().unwrap_or(json!("0x0000000000000000000000000000000000000000000000000000000000000000")),
                    });
                }
                let mut data = json!({
                    "approval_request": approval.clone(),
                    "signature_receipt": approval["signature_receipt"],
                    "signature": "0xsigned-managed",
                    "signed_payload": {}
                });
                if let Some(signed_transaction) = approval
                    .get("signed_result")
                    .and_then(|result| result.get("signed_transaction"))
                    .and_then(Value::as_str)
                {
                    data["signed_transaction"] = json!(signed_transaction);
                }
                Ok(json!({ "status": "ok", "data": data }))
            }
            Some("approve_approval") => {
                let principal_id = required_test_str(request, "principal_id")?;
                let request_id = required_test_str(request, "request_id")?;
                let mut approvals = self.approvals.lock().await;
                let Some(approval) = approvals.iter_mut().find(|approval| {
                    approval
                        .get("principal_id")
                        .and_then(|value| value.as_str())
                        == Some(principal_id)
                        && approval.get("request_id").and_then(|value| value.as_str())
                            == Some(request_id)
                }) else {
                    return Ok(json!({
                        "status": "error",
                        "code": "not_found",
                        "message": "wallet approval request not found"
                    }));
                };
                approval["status"] = serde_json::Value::String("approved".to_string());
                let payload_hash = approval
                    .get("payload_hash")
                    .and_then(|value| value.as_str())
                    .unwrap_or(
                        "0x0000000000000000000000000000000000000000000000000000000000000000",
                    );
                let signer = approval
                    .get("address")
                    .and_then(|value| value.as_str())
                    .unwrap_or("0xabc");
                let handoff = if approval.get("intent").and_then(|value| value.as_str())
                    == Some("transaction_intent")
                {
                    json!({
                        "schema": "elastos.wallet.webconnect_handoff/v1",
                        "request_id": request_id,
                        "intent": approval.get("intent").cloned().unwrap_or(json!("transaction_intent")),
                        "payload_hash": payload_hash,
                        "signer": signer,
                        "transaction": {
                            "from": signer,
                            "to": "0x2222222222222222222222222222222222222222",
                            "value": "0x1",
                            "data": "0x",
                            "gas": "0x5208",
                            "gasPrice": "0x3b9aca00",
                            "nonce": "0x1",
                            "chainId": "0x14"
                        },
                        "status": "awaiting_wallet_transaction"
                    })
                } else {
                    json!({
                        "schema": "elastos.wallet.webconnect_handoff/v1",
                        "request_id": request_id,
                        "intent": approval.get("intent").cloned().unwrap_or(json!("publish_envelope")),
                        "payload_hash": payload_hash,
                        "signer": signer,
                        "message": format!("ElastOS Wallet Approval\n\nRequest: {request_id}"),
                        "signature_type": "personal_sign",
                        "status": "awaiting_wallet_signature"
                    })
                };
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "approval_request": approval.clone(),
                        "handoff": handoff,
                        "signature": serde_json::Value::Null
                    }
                }))
            }
            Some("complete_approval") => {
                let principal_id = required_test_str(request, "principal_id")?;
                let request_id = required_test_str(request, "request_id")?;
                let connector_id = required_test_str(request, "connector_id")?;
                let mut approvals = self.approvals.lock().await;
                let Some(approval) = approvals.iter_mut().find(|approval| {
                    approval
                        .get("principal_id")
                        .and_then(|value| value.as_str())
                        == Some(principal_id)
                        && approval.get("request_id").and_then(|value| value.as_str())
                            == Some(request_id)
                }) else {
                    return Ok(json!({
                        "status": "error",
                        "code": "not_found",
                        "message": "wallet approval request not found"
                    }));
                };
                if approval
                    .get("connector_id")
                    .and_then(|value| value.as_str())
                    != Some(connector_id)
                {
                    return Ok(json!({
                        "status": "error",
                        "code": "invalid_request",
                        "message": "wallet approval request belongs to a different connector"
                    }));
                }
                approval["status"] = serde_json::Value::String("completed".to_string());
                if approval.get("intent").and_then(|value| value.as_str())
                    == Some("transaction_intent")
                {
                    approval["signed_result"] = json!({
                        "schema": "elastos.wallet.external-transaction-result/v1",
                        "request_id": request_id,
                        "method": "eth_sendTransaction",
                        "transaction_hash": required_test_str(request, "transaction_hash")?,
                        "signer": required_test_str(request, "signer")?,
                        "chain_namespace": approval.get("chain_namespace").cloned().unwrap_or(json!("eip155:20")),
                        "payload_hash": required_test_str(request, "payload_hash")?,
                    });
                }
                Ok(json!({
                    "status": "ok",
                    "data": {
                        "approval_request": approval.clone(),
                        "signature_receipt": {
                            "schema": "elastos.wallet.signature_receipt/v1",
                            "request_id": request_id,
                            "signer": required_test_str(request, "signer")?,
                            "payload_hash": required_test_str(request, "payload_hash")?,
                            "signature_hash": "0xcccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                            "completed_at": crate::auth::now_ts()
                        }
                    }
                }))
            }
            Some("sign_approved") => {
                let principal_id = required_test_str(request, "principal_id")?;
                let request_id = required_test_str(request, "request_id")?;
                let mut approvals = self.approvals.lock().await;
                let Some(approval) = approvals.iter_mut().find(|approval| {
                    approval
                        .get("principal_id")
                        .and_then(|value| value.as_str())
                        == Some(principal_id)
                        && approval.get("request_id").and_then(|value| value.as_str())
                            == Some(request_id)
                }) else {
                    return Ok(json!({
                        "status": "error",
                        "code": "not_found",
                        "message": "wallet approval request not found"
                    }));
                };
                if approval.get("status").and_then(|value| value.as_str()) != Some("approved") {
                    return Ok(json!({
                        "status": "error",
                        "code": "invalid_request",
                        "message": "wallet approval request must be approved before managed signing"
                    }));
                }
                approval["status"] = serde_json::Value::String("completed".to_string());
                approval["signature_receipt"] = json!({
                    "schema": "elastos.wallet.signature_receipt/v1",
                    "request_id": request_id,
                    "signer": approval.get("address").cloned().unwrap_or(json!("0x0")),
                    "payload_hash": approval.get("payload_hash").cloned().unwrap_or(json!("0x0000000000000000000000000000000000000000000000000000000000000000")),
                    "signature_hash": "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "completed_at": crate::auth::now_ts(),
                });
                if approval.get("intent").and_then(|value| value.as_str())
                    == Some("transaction_intent")
                {
                    let signed_transaction = mock_sign_eip155_transaction(
                        approval.get("payload").ok_or_else(|| {
                            ProviderError::Provider(
                                "mock transaction approval is missing payload".to_string(),
                            )
                        })?,
                    )?;
                    let transaction_hash =
                        signed_evm_transaction_hash_for_test(&signed_transaction);
                    approval["signed_result"] = json!({
                        "schema": "elastos.wallet.signed-transaction-result/v1",
                        "request_id": request_id,
                        "method": "eth_sendTransaction",
                        "signed_transaction": signed_transaction,
                        "transaction_hash": transaction_hash,
                        "signer": approval.get("address").cloned().unwrap_or(json!("0x0")),
                        "chain_namespace": approval.get("chain_namespace").cloned().unwrap_or(json!("eip155:20")),
                        "payload_hash": approval.get("payload_hash").cloned().unwrap_or(json!("0x0000000000000000000000000000000000000000000000000000000000000000")),
                    });
                }
                let mut data = json!({
                    "approval_request": approval.clone(),
                    "signature_receipt": approval["signature_receipt"],
                    "signature": "0xsigned-managed",
                    "signed_payload": {}
                });
                if let Some(signed_transaction) = approval
                    .get("signed_result")
                    .and_then(|result| result.get("signed_transaction"))
                    .and_then(|value| value.as_str())
                {
                    data["signed_transaction"] = json!(signed_transaction);
                }
                Ok(json!({
                    "status": "ok",
                    "data": data
                }))
            }
            Some("attach_validated_chain_outcome") => {
                let principal_id = required_test_str(request, "principal_id")?;
                let outcome = request
                    .get("outcome")
                    .filter(|value| value.is_object())
                    .cloned()
                    .ok_or_else(|| {
                        ProviderError::Provider("missing validated Chain outcome".to_string())
                    })?;
                let request_id = required_test_str(&outcome, "approval_request_id")?;
                let mut approvals = self.approvals.lock().await;
                let Some(approval) = approvals.iter_mut().find(|approval| {
                    approval
                        .get("principal_id")
                        .and_then(|value| value.as_str())
                        == Some(principal_id)
                        && approval.get("request_id").and_then(|value| value.as_str())
                            == Some(request_id)
                }) else {
                    return Ok(json!({
                        "status": "error",
                        "code": "not_found",
                        "message": "wallet approval request not found"
                    }));
                };
                if approval
                    .get("reason")
                    .and_then(Value::as_str)
                    .is_some_and(|reason| reason.contains("record-fails"))
                    && approval.get("projection_failed_once").is_none()
                {
                    approval["projection_failed_once"] = json!(true);
                    return Ok(json!({
                        "status": "error",
                        "code": "projection_failed",
                        "message": "simulated Wallet Chain outcome projection failure"
                    }));
                }
                if approval.get("status").and_then(|value| value.as_str()) != Some("completed") {
                    return Ok(json!({
                        "status": "error",
                        "code": "invalid_request",
                        "message": "Wallet Chain outcome requires completed approval"
                    }));
                }
                if approval.get("intent").and_then(|value| value.as_str())
                    != Some("transaction_intent")
                {
                    return Ok(json!({
                        "status": "error",
                        "code": "invalid_request",
                        "message": "wallet approval request is not a transaction"
                    }));
                }
                if let Some(existing) = approval.get("validated_chain_outcome") {
                    if existing != &outcome {
                        return Ok(json!({
                            "status": "error",
                            "code": "chain_outcome_conflict",
                            "message": "simulated Wallet Chain outcome substitution"
                        }));
                    }
                } else {
                    approval["validated_chain_outcome"] = outcome;
                }
                Ok(json!({
                    "status": "ok",
                    "data": { "approval_request": approval.clone() }
                }))
            }
            _ => Ok(json!({
                "status": "error",
                "code": "unsupported",
                "message": "unsupported mock wallet op"
            })),
        }
    }
}
