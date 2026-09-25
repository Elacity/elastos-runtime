//! Protected-content Market Buy dataset.
//!
//! Buy and read are separate workflows with separate datasets (D1): the
//! access token exists on chain whether or not a file is retrievable, and
//! read-side checks answer "can I open it" while buy-side checks answer "can
//! I buy it". This module holds the buy-side dataset only -- the item a
//! chain identifies (`RuntimeMarketItem`), the offer a seller posted
//! (`RuntimeMarketOffer`), and the owner-only record of what a principal
//! bought on the market (`RuntimeMarketPurchaseRecord`). It never runs
//! read-side checks and the read dataset in `protected_content_runtime`
//! never runs these.
//!
//! Its consumers are the listing endpoint (`api::gateway_marketplace_listing`)
//! and `buy_offer` (`api::gateway_provider_proxy`).
//!
//! The one bridge between the two datasets is adoption (D10): after a market
//! purchase completes, [`shared_read_dataset`] reads the read dataset out of
//! the asset's shared `metadata.json` -- never `manifest.json` -- and the
//! runtime turns it into the records the unchanged open path consumes.

#[cfg(test)]
mod tests;

use std::io::Read as _;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::{Mutex as StdMutex, OnceLock};

use elastos_protected_content_runtime::ExclusiveFileLock;
use serde::{Deserialize, Serialize};
use sha2::Digest as _;

use elastos_protected_content_provider_contracts::ELASTOS_PQ_PROTECTION_SCHEME_V1;
use elastos_protected_content_runtime::RuntimeContentIdentityV1;

use crate::protected_content_runtime::{
    decode_runtime_shared_content_identity, ensure_owner_only_runtime_storage_parent,
    load_runtime_custody_purchase_for_chain_item, open_owner_only_runtime_record_file,
    persist_runtime_custody_purchase, retire_runtime_custody_purchase_attempt,
    validate_runtime_custody_canonical_quantity, validate_runtime_custody_evm_address,
    write_owner_only_bytes, RuntimeCustodyPurchaseProgress, RuntimeCustodyPurchaseRecord,
    RuntimeCustodyPurchaseStageRecord, RuntimePortableListingPackage,
};

/// A buy request's item identity, keyed exactly as the chain keys it (D2):
/// `(chain_namespace, ledger, token_id)`. The KID is an alias, trusted only
/// after `ipReference(kid) == (ledger, token_id)` -- a check this module
/// does not perform; it only shapes and bounds the field.
///
/// The KID is optional (R46): a purchase rests on the chain's terms alone, so
/// `buy_offer` records an item whose KID nobody has proven yet as `None`, and
/// adoption fills it in once the shared document and the chain agree on it.
/// A record written before R46 names its KID, and still loads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuntimeMarketItem {
    pub(crate) chain_namespace: String, // "eip155:<id>"
    pub(crate) network: String,
    pub(crate) ledger: String,   // lowercase 0x + 40 hex
    pub(crate) token_id: String, // canonical hex quantity
    pub(crate) operative: String,
    /// `0x` + 32 hex, verified via ipReference; `None` while unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) kid: Option<String>,
}

/// The terms of one seller's live offer for a `RuntimeMarketItem`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuntimeMarketOffer {
    pub(crate) seller: String,
    pub(crate) price: String,
    pub(crate) pay_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) payment_processor: Option<String>,
    pub(crate) quantity: String, // agreed quantity, "0x1" in this slice
}

const RUNTIME_MARKET_ITEM_INVALID_MESSAGE: &str = "Runtime market item is invalid";
const RUNTIME_MARKET_OFFER_INVALID_MESSAGE: &str = "Runtime market offer is invalid";

/// Bound for `chain_namespace` / `network`, matching the bound
/// `protected_content_runtime`'s own public-text fields use.
const MAX_RUNTIME_MARKET_TEXT_BYTES: usize = 256;

fn validate_runtime_market_text(value: &str) -> anyhow::Result<()> {
    if value.is_empty()
        || value.len() > MAX_RUNTIME_MARKET_TEXT_BYTES
        || value.chars().any(char::is_control)
    {
        anyhow::bail!(RUNTIME_MARKET_ITEM_INVALID_MESSAGE);
    }
    Ok(())
}

/// `kid` is a `bytes16` on chain (`ipReference(bytes16)`): `0x` followed by
/// exactly 32 lowercase hex digits.
fn validate_runtime_market_kid(value: &str) -> anyhow::Result<()> {
    let raw = value
        .strip_prefix("0x")
        .ok_or_else(|| anyhow::anyhow!(RUNTIME_MARKET_ITEM_INVALID_MESSAGE))?;
    if raw.len() != 32
        || !raw
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        anyhow::bail!(RUNTIME_MARKET_ITEM_INVALID_MESSAGE);
    }
    Ok(())
}

impl RuntimeMarketItem {
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        validate_runtime_market_text(&self.chain_namespace)?;
        self.chain_id()?;
        validate_runtime_market_text(&self.network)?;
        validate_runtime_custody_evm_address(&self.ledger)
            .map_err(|_| anyhow::anyhow!(RUNTIME_MARKET_ITEM_INVALID_MESSAGE))?;
        validate_runtime_custody_canonical_quantity(&self.token_id)
            .map_err(|_| anyhow::anyhow!(RUNTIME_MARKET_ITEM_INVALID_MESSAGE))?;
        validate_runtime_custody_evm_address(&self.operative)
            .map_err(|_| anyhow::anyhow!(RUNTIME_MARKET_ITEM_INVALID_MESSAGE))?;
        if let Some(kid) = &self.kid {
            validate_runtime_market_kid(kid)?;
        }
        Ok(())
    }

    /// The same item the chain keys, with the same operative -- whatever
    /// either side knows of its KID. A KID learned after the attempt was
    /// recorded does not make it another item.
    pub(crate) fn same_item_as(&self, other: &RuntimeMarketItem) -> bool {
        self.chain_namespace == other.chain_namespace
            && self.network == other.network
            && self.ledger.eq_ignore_ascii_case(&other.ledger)
            && self.token_id.eq_ignore_ascii_case(&other.token_id)
            && self.operative.eq_ignore_ascii_case(&other.operative)
    }

    /// The `eip155` chain id `chain_namespace` names.
    pub(crate) fn chain_id(&self) -> anyhow::Result<u64> {
        self.chain_namespace
            .strip_prefix("eip155:")
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| anyhow::anyhow!(RUNTIME_MARKET_ITEM_INVALID_MESSAGE))
    }

    /// "<chain_id>-<ledger>-<token_id>" -- the record file stem.
    ///
    /// Validates first: `ledger` and `token_id` are embedded verbatim into a
    /// filesystem path by every caller of this stem, so an unvalidated item
    /// (deserialized from a request, where any string is accepted) could
    /// otherwise smuggle `/` or `..` into the path. `validate()` restricts
    /// both to their canonical hex shapes, which cannot contain either.
    pub(crate) fn record_stem(&self) -> anyhow::Result<String> {
        self.validate()?;
        let chain_id = self.chain_id()?;
        Ok(format!("{chain_id}-{}-{}", self.ledger, self.token_id))
    }
}

impl RuntimeMarketOffer {
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        validate_runtime_custody_evm_address(&self.seller)
            .map_err(|_| anyhow::anyhow!(RUNTIME_MARKET_OFFER_INVALID_MESSAGE))?;
        validate_runtime_custody_canonical_quantity(&self.price)
            .map_err(|_| anyhow::anyhow!(RUNTIME_MARKET_OFFER_INVALID_MESSAGE))?;
        validate_runtime_custody_evm_address(&self.pay_token)
            .map_err(|_| anyhow::anyhow!(RUNTIME_MARKET_OFFER_INVALID_MESSAGE))?;
        if let Some(payment_processor) = &self.payment_processor {
            validate_runtime_custody_evm_address(payment_processor)
                .map_err(|_| anyhow::anyhow!(RUNTIME_MARKET_OFFER_INVALID_MESSAGE))?;
        }
        validate_runtime_custody_canonical_quantity(&self.quantity)
            .map_err(|_| anyhow::anyhow!(RUNTIME_MARKET_OFFER_INVALID_MESSAGE))?;
        Ok(())
    }

    /// The terms a buyer agreed to, compared with a fresh read. Availability
    /// (`quantity`) is not a term: a seller topping up or partially selling
    /// down does not change what the buyer agreed to pay, so a buy is only
    /// ever answered `terms_changed` for seller, price or pay token moving,
    /// or for the offer's quantity dropping to `0x0` -- which the caller
    /// checks separately from this comparison.
    pub(crate) fn same_terms_as(&self, fresh: &RuntimeMarketOffer) -> bool {
        self.seller.eq_ignore_ascii_case(&fresh.seller)
            && self.price == fresh.price
            && self.pay_token.eq_ignore_ascii_case(&fresh.pay_token)
    }
}

/// Both halves of today's package, for the listing-package buy path.
pub(crate) fn market_terms_from_package(
    package: &RuntimePortableListingPackage,
    kid: &str,
) -> (RuntimeMarketItem, RuntimeMarketOffer) {
    let item = RuntimeMarketItem {
        chain_namespace: package.chain_namespace.clone(),
        network: package.network.clone(),
        ledger: package.ledger.clone(),
        token_id: package.token_id.clone(),
        operative: package.operative.clone(),
        kid: Some(kid.to_string()),
    };
    let offer = RuntimeMarketOffer {
        seller: package.seller_address.clone(),
        price: package.price.clone(),
        pay_token: package.pay_token.clone(),
        payment_processor: package.payment_processor.clone(),
        quantity: package.quantity.clone(),
    };
    (item, offer)
}

/// The item a `buy_offer` request names, exactly as the page sends it
/// (§5.2, R20). No `operative`: Runtime derives it. `kid` is optional and
/// gates nothing (R46): the purchase rests on the chain's terms alone.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuntimeMarketBuyItemClaim {
    pub(crate) chain_namespace: String,
    pub(crate) network: String,
    pub(crate) ledger: String,
    pub(crate) token_id: String,
    #[serde(default)]
    pub(crate) kid: Option<String>,
}

/// The terms the buyer agreed to, exactly as the page sends them (§5.2, R20).
/// The seller is named once, at the top of the request.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuntimeMarketAgreedTerms {
    pub(crate) price: String,
    pub(crate) pay_token: String,
    pub(crate) quantity: String,
}

/// One `buy_offer` request, as the gateway hands it on: the principal the
/// gateway verified, and everything else exactly as the page claimed it.
/// Nothing here is trusted until `buy_offer` has re-derived it; the
/// validated `RuntimeMarketItem` / `RuntimeMarketOffer` are built only after
/// the binding check.
#[derive(Debug, Clone)]
pub(crate) struct RuntimeMarketBuyInput {
    pub(crate) principal_id: String,
    pub(crate) item: RuntimeMarketBuyItemClaim,
    pub(crate) asset_uri: String,
    pub(crate) seller: String,
    pub(crate) agreed: RuntimeMarketAgreedTerms,
}

/// A new attempt's identity: 16 bytes from the OS entropy source, as 32
/// lowercase hex digits (R23).
pub(crate) fn new_runtime_market_attempt_id() -> anyhow::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).map_err(|error| {
        anyhow::anyhow!("OS randomness unavailable for a market attempt: {error}")
    })?;
    Ok(hex::encode(bytes))
}

fn is_runtime_market_attempt_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Wire identity of a market purchase record.
pub(crate) const RUNTIME_MARKET_PURCHASE_SCHEMA_V1: &str = "elastos.library.market-purchase/v1";

const RUNTIME_MARKET_PURCHASE_ROOT: &str = "protected-content/market-purchases";

/// A principal's record of one market purchase, keyed by item (D9): buy-side
/// facts only, entirely separate from the read-side listing and purchase
/// records in `protected_content_runtime`.
// No `Eq`: `RuntimeCustodyPurchaseStageRecord` / `RuntimeCustodyPurchaseProgress`
// carry a `serde_json::Value` and only derive `PartialEq`, exactly like the
// read-side `RuntimeCustodyPurchaseRecord` they were copied from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuntimeMarketPurchaseRecord {
    pub(crate) schema: String,
    pub(crate) principal_id: String,
    pub(crate) account_id: String,
    pub(crate) address: String,
    pub(crate) item: RuntimeMarketItem,
    pub(crate) asset_uri: String,
    pub(crate) offer: RuntimeMarketOffer,
    /// This attempt's own identity: 16 random bytes, lowercase hex, drawn
    /// when the attempt is first recorded (R23). Bound into the market
    /// purchase request hash, so a retry after a reverted buy -- even at the
    /// very same terms -- is a new effect, never the spent one; a resume
    /// reuses it, so a second press is the same effect.
    pub(crate) attempt_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) approval_stage: Option<RuntimeCustodyPurchaseStageRecord>,
    pub(crate) acquisition_stage: RuntimeCustodyPurchaseStageRecord,
    pub(crate) progress: RuntimeCustodyPurchaseProgress,
    /// Set once adoption has produced the read-side records. `None` for a
    /// foreign asset, or while adoption has not yet succeeded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) adopted_mint_id: Option<String>,
    pub(crate) created_at: u64,
    pub(crate) updated_at: u64,
}

fn runtime_market_purchase_principal_hash(principal_id: &str) -> String {
    let mut hasher = sha2::Sha256::new();
    hasher.update(principal_id.as_bytes());
    hex::encode(hasher.finalize())
}

/// `<data>/protected-content/market-purchases/<sha256(principal_id)>/<chain_id>-<ledger>-<token_id>.json`
///
/// Refuses before touching the filesystem: `item.validate()` first (belt),
/// then, once the path is built, an assertion that its parent is exactly the
/// per-principal directory this function computed (suspenders) -- catching
/// any future field or bug that could otherwise smuggle extra path
/// components (`/`, `..`) into the stem, the same way an unvalidated
/// `ledger`/`token_id` did before this fix.
pub(crate) fn runtime_market_purchase_path(
    data_dir: &Path,
    principal_id: &str,
    item: &RuntimeMarketItem,
) -> anyhow::Result<PathBuf> {
    item.validate()?;
    runtime_market_purchase_stem_path(data_dir, principal_id, &item.record_stem()?)
}

fn runtime_market_purchase_stem_path(
    data_dir: &Path,
    principal_id: &str,
    stem: &str,
) -> anyhow::Result<PathBuf> {
    let principal_dir = data_dir
        .join(RUNTIME_MARKET_PURCHASE_ROOT)
        .join(runtime_market_purchase_principal_hash(principal_id));
    let path = principal_dir.join(format!("{stem}.json"));
    if path.parent() != Some(principal_dir.as_path()) {
        anyhow::bail!("Runtime market purchase is invalid");
    }
    Ok(path)
}

/// The record path for the item the chain keys `(chain_namespace, ledger,
/// token_id)` (D2), for a caller that knows neither its operative nor its
/// KID. The key is canonicalized (lowercase) and validated exactly as
/// `RuntimeMarketItem::validate` validates it before it reaches a path.
fn runtime_market_purchase_key_path(
    data_dir: &Path,
    principal_id: &str,
    chain_namespace: &str,
    ledger: &str,
    token_id: &str,
) -> anyhow::Result<PathBuf> {
    let ledger = ledger.to_ascii_lowercase();
    let token_id = token_id.to_ascii_lowercase();
    validate_runtime_custody_evm_address(&ledger)
        .map_err(|_| anyhow::anyhow!(RUNTIME_MARKET_ITEM_INVALID_MESSAGE))?;
    validate_runtime_custody_canonical_quantity(&token_id)
        .map_err(|_| anyhow::anyhow!(RUNTIME_MARKET_ITEM_INVALID_MESSAGE))?;
    let chain_id = chain_namespace
        .strip_prefix("eip155:")
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| anyhow::anyhow!(RUNTIME_MARKET_ITEM_INVALID_MESSAGE))?;
    runtime_market_purchase_stem_path(
        data_dir,
        principal_id,
        &format!("{chain_id}-{ledger}-{token_id}"),
    )
}

/// The lock file guarding a principal's market-purchase directory, mirroring
/// `runtime_purchase_lock_path`'s discipline for the read-side purchase
/// ledger: every `persist_runtime_market_purchase` call for this principal
/// serializes through this lock before writing.
fn runtime_market_purchase_lock_path(data_dir: &Path, principal_id: &str) -> PathBuf {
    data_dir
        .join(RUNTIME_MARKET_PURCHASE_ROOT)
        .join(runtime_market_purchase_principal_hash(principal_id))
        .join(".lock")
}

/// Test-only observability for the market-purchase lock, mirroring
/// `protected_content_runtime`'s purchase-ledger lock probe: proves
/// `persist_runtime_market_purchase` callers for the *same* lock path never
/// hold their critical section concurrently. Compiled out entirely outside
/// `cfg(test)`.
#[cfg(test)]
static RUNTIME_MARKET_PURCHASE_LOCK_TEST_STATE: OnceLock<
    StdMutex<std::collections::HashMap<PathBuf, (usize, bool)>>,
> = OnceLock::new();

#[cfg(test)]
fn runtime_market_purchase_lock_test_state(
) -> &'static StdMutex<std::collections::HashMap<PathBuf, (usize, bool)>> {
    RUNTIME_MARKET_PURCHASE_LOCK_TEST_STATE
        .get_or_init(|| StdMutex::new(std::collections::HashMap::new()))
}

#[cfg(test)]
struct RuntimeMarketPurchaseLockTestProbe {
    path: PathBuf,
}

#[cfg(test)]
impl RuntimeMarketPurchaseLockTestProbe {
    fn enter(path: PathBuf) -> Self {
        {
            let mut state = runtime_market_purchase_lock_test_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let entry = state.entry(path.clone()).or_insert((0, false));
            entry.0 += 1;
            if entry.0 > 1 {
                entry.1 = true;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        Self { path }
    }
}

#[cfg(test)]
impl Drop for RuntimeMarketPurchaseLockTestProbe {
    fn drop(&mut self) {
        let mut state = runtime_market_purchase_lock_test_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(entry) = state.get_mut(&self.path) {
            entry.0 -= 1;
        }
    }
}

#[cfg(test)]
pub(crate) fn runtime_market_purchase_lock_test_overlap_detected(path: &Path) -> bool {
    runtime_market_purchase_lock_test_state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(path)
        .is_some_and(|(_, overlapped)| *overlapped)
}

/// Refuse before any filesystem access: `record.item` is embedded into the
/// record's own file path (via `runtime_market_purchase_path`), `record.offer`
/// carries the agreed terms and `record.attempt_id` the attempt's identity,
/// so all are validated before anything is created or written.
fn validate_runtime_market_purchase_for_write(
    record: &RuntimeMarketPurchaseRecord,
) -> anyhow::Result<()> {
    record.item.validate()?;
    record.offer.validate()?;
    if !is_runtime_market_attempt_id(&record.attempt_id) {
        anyhow::bail!("Runtime market purchase is invalid");
    }
    Ok(())
}

/// Run `critical` holding the principal's exclusive market-purchase lock.
/// Every write, create and retire goes through here, so a check made inside
/// `critical` still holds when it acts -- across threads and processes.
fn with_runtime_market_purchase_lock<T>(
    data_dir: &Path,
    principal_id: &str,
    critical: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let lock_path = runtime_market_purchase_lock_path(data_dir, principal_id);
    #[cfg(unix)]
    if let Some(parent) = lock_path.parent() {
        ensure_owner_only_runtime_storage_parent(parent)?;
    }
    #[cfg(not(unix))]
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _lock = ExclusiveFileLock::acquire(&lock_path)?;
    #[cfg(test)]
    let _lock_test_probe = RuntimeMarketPurchaseLockTestProbe::enter(lock_path.clone());
    critical()
}

fn write_runtime_market_purchase(
    data_dir: &Path,
    record: &RuntimeMarketPurchaseRecord,
) -> anyhow::Result<()> {
    let path = runtime_market_purchase_path(data_dir, &record.principal_id, &record.item)?;
    write_owner_only_bytes(&path, &serde_json::to_vec(record)?)
}

/// Write `record` unconditionally -- a test fixture's way to seed a record.
/// `buy_offer` never writes this way: it creates with
/// [`create_runtime_market_purchase`] and advances with
/// [`update_runtime_market_purchase`], both conditional on the attempt.
#[cfg(test)]
pub(crate) fn persist_runtime_market_purchase(
    data_dir: &Path,
    record: &RuntimeMarketPurchaseRecord,
) -> anyhow::Result<()> {
    validate_runtime_market_purchase_for_write(record)?;
    with_runtime_market_purchase_lock(data_dir, &record.principal_id, || {
        write_runtime_market_purchase(data_dir, record)
    })
}

/// What creating a new attempt found (R26, R29).
#[derive(Debug)]
pub(crate) enum RuntimeMarketCreateOutcome {
    /// No attempt was recorded; `record` now is.
    Created,
    /// Another press recorded an attempt first. Nothing was written; the
    /// caller resumes this one, so two presses are one effect.
    Existing(Box<RuntimeMarketPurchaseRecord>),
    /// The same item is already being bought -- or was bought -- through the
    /// listing-package path (`buy`). Nothing was written: one item is one
    /// purchase, whichever path started it (R29).
    ListingPurchase(Box<RuntimeCustodyPurchaseRecord>),
}

/// Record a NEW attempt, create-only (R26): under the principal's lock, an
/// attempt already recorded for the item wins and is returned untouched, and
/// a listing-package purchase of the same item refuses it (R29). The
/// listing-package path creates its records under this same lock
/// ([`create_runtime_listing_purchase_unless_market`]), so neither path can
/// start a purchase the other has started, however the two interleave.
pub(crate) fn create_runtime_market_purchase(
    data_dir: &Path,
    record: &RuntimeMarketPurchaseRecord,
) -> anyhow::Result<RuntimeMarketCreateOutcome> {
    validate_runtime_market_purchase_for_write(record)?;
    with_runtime_market_purchase_lock(data_dir, &record.principal_id, || {
        if let Some(existing) =
            load_runtime_market_purchase(data_dir, &record.principal_id, &record.item)?
        {
            return Ok(RuntimeMarketCreateOutcome::Existing(Box::new(existing)));
        }
        if let Some(listing_purchase) = load_runtime_custody_purchase_for_chain_item(
            data_dir,
            &record.principal_id,
            &record.item.chain_namespace,
            &record.item.ledger,
            &record.item.token_id,
        )? {
            return Ok(RuntimeMarketCreateOutcome::ListingPurchase(Box::new(
                listing_purchase,
            )));
        }
        write_runtime_market_purchase(data_dir, record)?;
        Ok(RuntimeMarketCreateOutcome::Created)
    })
}

/// Record a NEW listing-package purchase (`buy`), unless a market purchase of
/// the same item is recorded (R29). Runs under the market-purchase lock that
/// [`create_runtime_market_purchase`] holds, so the two paths' creates are
/// serialized. `Ok(Some(existing))` names the market purchase that refused it;
/// nothing was written.
pub(crate) fn create_runtime_listing_purchase_unless_market(
    data_dir: &Path,
    purchase: &RuntimeCustodyPurchaseRecord,
) -> anyhow::Result<Option<RuntimeMarketPurchaseRecord>> {
    with_runtime_market_purchase_lock(data_dir, &purchase.principal_id, || {
        if let Some(existing) = load_runtime_market_purchase_by_key(
            data_dir,
            &purchase.principal_id,
            &purchase.chain_namespace,
            &purchase.ledger,
            &purchase.token_id,
        )? {
            return Ok(Some(existing));
        }
        persist_runtime_custody_purchase(data_dir, purchase)?;
        Ok(None)
    })
}

/// Retire a listing-package purchase (`buy`) that is provably over without
/// payment -- its Wallet approval was declined, or its buy was mined and
/// reverted (R41). Held under this market-purchase lock, the same one R29's
/// cross-path checks run under, and only while the record is still `attempt`
/// ([`retire_runtime_custody_purchase_attempt`]), so a newer attempt is never
/// removed. `Ok(false)` when there was nothing of this attempt's to retire.
pub(crate) fn retire_runtime_listing_purchase(
    data_dir: &Path,
    attempt: &RuntimeCustodyPurchaseRecord,
) -> anyhow::Result<bool> {
    with_runtime_market_purchase_lock(data_dir, &attempt.principal_id, || {
        retire_runtime_custody_purchase_attempt(data_dir, attempt)
    })
}

/// Advance an attempt's record (R26): written only if the record on disk is
/// still THIS attempt. `Ok(false)` when it was retired or superseded by a
/// newer attempt -- which is then left exactly as it is.
pub(crate) fn update_runtime_market_purchase(
    data_dir: &Path,
    record: &RuntimeMarketPurchaseRecord,
) -> anyhow::Result<bool> {
    validate_runtime_market_purchase_for_write(record)?;
    with_runtime_market_purchase_lock(data_dir, &record.principal_id, || {
        match load_runtime_market_purchase(data_dir, &record.principal_id, &record.item)? {
            Some(current) if current.attempt_id == record.attempt_id => {
                write_runtime_market_purchase(data_dir, record)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    })
}

pub(crate) fn load_runtime_market_purchase(
    data_dir: &Path,
    principal_id: &str,
    item: &RuntimeMarketItem,
) -> anyhow::Result<Option<RuntimeMarketPurchaseRecord>> {
    // Refuse before any filesystem access, same as `persist` above.
    item.validate()?;
    let path = runtime_market_purchase_path(data_dir, principal_id, item)?;
    if !path.exists() {
        return Ok(None);
    }
    let mut file = open_owner_only_runtime_record_file(&path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let record: RuntimeMarketPurchaseRecord = serde_json::from_slice(&bytes)?;
    if record.schema != RUNTIME_MARKET_PURCHASE_SCHEMA_V1 || record.principal_id != principal_id {
        anyhow::bail!("Runtime market purchase is invalid");
    }
    Ok(Some(record))
}

/// This principal's market purchase of the item keyed `(chain_namespace,
/// ledger, token_id)` (D2), whatever its progress: the record a
/// listing-package purchase of the same item must not race (R29).
pub(crate) fn load_runtime_market_purchase_by_key(
    data_dir: &Path,
    principal_id: &str,
    chain_namespace: &str,
    ledger: &str,
    token_id: &str,
) -> anyhow::Result<Option<RuntimeMarketPurchaseRecord>> {
    let path = runtime_market_purchase_key_path(
        data_dir,
        principal_id,
        chain_namespace,
        ledger,
        token_id,
    )?;
    if !path.exists() {
        return Ok(None);
    }
    let mut file = open_owner_only_runtime_record_file(&path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let record: RuntimeMarketPurchaseRecord = serde_json::from_slice(&bytes)?;
    if record.schema != RUNTIME_MARKET_PURCHASE_SCHEMA_V1
        || record.principal_id != principal_id
        || record.item.chain_namespace != chain_namespace
        || !record.item.ledger.eq_ignore_ascii_case(ledger)
        || !record.item.token_id.eq_ignore_ascii_case(token_id)
    {
        anyhow::bail!("Runtime market purchase is invalid");
    }
    Ok(Some(record))
}

/// Retire a principal's market purchase for `item`: remove its record, under
/// the same validated path, owner-only checks and exclusive lock as a write
/// -- and only if the record is still the attempt `attempt_id` (R26).
///
/// Only for an attempt that is provably dead: its buy was mined and
/// reverted, or its Wallet approval was declined, so its effect can never
/// land. An attempt still waiting on its approval or its broadcast is alive
/// and must keep resuming. `Ok(false)` when the record is gone or is another
/// (newer) attempt, which is then left exactly as it is.
pub(crate) fn retire_runtime_market_purchase(
    data_dir: &Path,
    principal_id: &str,
    item: &RuntimeMarketItem,
    attempt_id: &str,
) -> anyhow::Result<bool> {
    item.validate()?;
    with_runtime_market_purchase_lock(data_dir, principal_id, || {
        // The load refuses a symlink or a hard-linked file exactly as any
        // read does, so a retirement can never be pointed at anything but
        // this record.
        match load_runtime_market_purchase(data_dir, principal_id, item)? {
            Some(current) if current.attempt_id == attempt_id => {}
            _ => return Ok(false),
        }
        let path = runtime_market_purchase_path(data_dir, principal_id, item)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    })
}

/// Every completed market purchase this principal holds. Unreadable records
/// are skipped: this answers a display question (the catalog's access state),
/// and a record that cannot be read is not one this Home can stand behind.
pub(crate) fn completed_runtime_market_purchases(
    data_dir: &Path,
    principal_id: &str,
) -> Vec<RuntimeMarketPurchaseRecord> {
    let directory = data_dir
        .join(RUNTIME_MARKET_PURCHASE_ROOT)
        .join(runtime_market_purchase_principal_hash(principal_id));
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return Vec::new();
    };
    let mut completed = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let record = open_owner_only_runtime_record_file(&path)
            .ok()
            .and_then(|mut file| {
                let mut bytes = Vec::new();
                file.read_to_end(&mut bytes).ok()?;
                serde_json::from_slice::<RuntimeMarketPurchaseRecord>(&bytes).ok()
            })
            .filter(|record| {
                record.schema == RUNTIME_MARKET_PURCHASE_SCHEMA_V1
                    && record.principal_id == principal_id
                    && record.item.validate().is_ok()
            });
        match record {
            Some(record)
                if matches!(
                    record.progress,
                    RuntimeCustodyPurchaseProgress::Complete { .. }
                ) =>
            {
                completed.push(record)
            }
            Some(_) => {}
            // A record this Home wrote and can no longer read is damage, not
            // a purchase in progress: said where an operator will see it.
            None => tracing::warn!(
                path = %path.display(),
                "market purchase record unreadable; skipped"
            ),
        }
    }
    completed
}

/// This principal's COMPLETED market purchase of the item keyed `(chain, ledger,
/// token_id)` (D2), for a caller that names an item the way the chain does
/// and so knows neither its operative nor its KID.
pub(crate) fn load_completed_runtime_market_purchase_by_key(
    data_dir: &Path,
    principal_id: &str,
    chain_namespace: &str,
    ledger: &str,
    token_id: &str,
) -> anyhow::Result<Option<RuntimeMarketPurchaseRecord>> {
    Ok(completed_runtime_market_purchases(data_dir, principal_id)
        .into_iter()
        .find(|record| {
            record.item.chain_namespace == chain_namespace
                && record.item.ledger.eq_ignore_ascii_case(ledger)
                && record.item.token_id.eq_ignore_ascii_case(token_id)
        }))
}

/// What adoption achieved for a completed market purchase. Never a purchase
/// failure: every arm leaves the purchase exactly as complete as it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AdoptionOutcome {
    /// The read-side records exist; the existing open path opens it.
    Adopted { mint_id: String },
    /// The shared document names no ElastOS protection at all: the asset
    /// opens where it was protected (for example on ela.city), not here.
    Foreign,
    /// The shared document names a KID the chain binds to another item, or
    /// one that contradicts the KID this purchase already proved (R46). It
    /// is refused as a foreign asset is: nothing is adopted from it.
    Mismatch,
    /// An ElastOS asset this Home could not adopt yet -- the document or the
    /// content was unreachable, or an entry it carries cannot be adopted. The
    /// reason is for the log only.
    NotYet(String),
}

impl AdoptionOutcome {
    /// The `adoption` a `buy_offer` completion answers with (R11).
    pub(crate) const fn wire_value(&self) -> &'static str {
        match self {
            Self::Adopted { .. } => "adopted",
            Self::Foreign | Self::Mismatch => "foreign",
            Self::NotYet(_) => "pending",
        }
    }
}

/// The read dataset, taken from the shared `metadata.json` only (D4).
#[derive(Debug, Clone)]
pub(crate) struct SharedReadDataset {
    /// `0x` + 32 lowercase hex (R9).
    pub(crate) kid: String,
    /// `media.uri` without `ipfs://`.
    pub(crate) content_cid: String,
    /// `media.mimeType`.
    pub(crate) mime_type: String,
    /// `name`.
    pub(crate) name: String,
    pub(crate) rights_policy_identity_base64: String,
    pub(crate) key_envelope_identity_base64: String,
    pub(crate) content_key_commitment_base64: String,
    /// `content_identity_base64`, decoded.
    pub(crate) content_identity: RuntimeContentIdentityV1,
    /// `did:pkh:eip155:<properties.chainId>:<properties.publisher>` (D16).
    pub(crate) publisher_identity: String,
}

/// Any protection scheme this Runtime speaks, current or not.
const ELASTOS_PROTECTION_SCHEME_PREFIX: &str = "cenc:elastos-";

/// R9: strip an optional `0x`, require exactly 32 hex digits, lowercase,
/// re-prefix `0x`.
pub(crate) fn normalize_runtime_market_kid(value: &str) -> Option<String> {
    let value = value.trim();
    let raw = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    (raw.len() == 32 && raw.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| format!("0x{}", raw.to_ascii_lowercase()))
}

fn shared_text(metadata: &serde_json::Value, pointer: &str) -> anyhow::Result<String> {
    metadata
        .pointer(pointer)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("shared metadata has no {pointer}"))
}

/// The read dataset from an asset's shared `metadata.json` (D4, R12).
///
/// `Ok(None)` when the document names no ElastOS protection at all -- neither
/// an `asset.protections[*].protectionType` nor a `media.protectionType[*]` in
/// the ElastOS scheme -- which makes the asset foreign. An ElastOS entry that
/// cannot be adopted (a `-v0` entry, or a `-v1` entry missing an identity) is
/// an error, not foreign: the asset is ours, just not adoptable.
///
/// `manifest.json` is never read; nothing here needs anything but the
/// document every marketplace reads.
pub(crate) fn shared_read_dataset(
    metadata: &serde_json::Value,
) -> anyhow::Result<Option<SharedReadDataset>> {
    let protections = metadata
        .pointer("/asset/protections")
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let scheme = |entry: &serde_json::Value| {
        entry
            .get("protectionType")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let identity = |entry: &serde_json::Value, field: &str| {
        entry
            .get(field)
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let adoptable = protections.iter().find_map(|entry| {
        if scheme(entry) != ELASTOS_PQ_PROTECTION_SCHEME_V1 {
            return None;
        }
        Some((
            identity(entry, "rights_policy_identity_base64")?,
            identity(entry, "key_envelope_identity_base64")?,
            identity(entry, "content_key_commitment_base64")?,
            identity(entry, "content_identity_base64")?,
        ))
    });
    let Some((rights_policy, key_envelope, content_key_commitment, content_identity)) = adoptable
    else {
        let media_schemes: Vec<String> = match metadata.pointer("/media/protectionType") {
            Some(serde_json::Value::Array(values)) => values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect(),
            Some(serde_json::Value::String(value)) => vec![value.clone()],
            _ => Vec::new(),
        };
        let elastos = protections
            .iter()
            .map(scheme)
            .chain(media_schemes)
            .any(|scheme| scheme.starts_with(ELASTOS_PROTECTION_SCHEME_PREFIX));
        if elastos {
            anyhow::bail!("the shared document's ElastOS protection cannot be adopted");
        }
        return Ok(None);
    };
    // R9: every KID the document states is normalized, and all of them must
    // name the same one.
    let mut kid: Option<String> = None;
    for pointer in ["/kid", "/properties/kid", "/asset/kid"] {
        let Some(value) = metadata.pointer(pointer) else {
            continue;
        };
        let named = value
            .as_str()
            .and_then(normalize_runtime_market_kid)
            .ok_or_else(|| anyhow::anyhow!("shared metadata names an invalid kid"))?;
        match &kid {
            Some(first) if *first != named => {
                anyhow::bail!("shared metadata names two kids")
            }
            Some(_) => {}
            None => kid = Some(named),
        }
    }
    let kid = kid.ok_or_else(|| anyhow::anyhow!("shared metadata names no kid"))?;
    let media_uri = shared_text(metadata, "/media/uri")?;
    let content_cid = media_uri
        .strip_prefix("ipfs://")
        .ok_or_else(|| anyhow::anyhow!("shared metadata media.uri is not ipfs://"))?
        .to_string();
    let publisher = shared_text(metadata, "/properties/publisher")?.to_ascii_lowercase();
    validate_runtime_custody_evm_address(&publisher)
        .map_err(|_| anyhow::anyhow!("shared metadata publisher is not an address"))?;
    let chain_id = match metadata.pointer("/properties/chainId") {
        Some(serde_json::Value::Number(number)) => number.as_u64(),
        Some(serde_json::Value::String(text)) => text.parse::<u64>().ok(),
        _ => None,
    }
    .ok_or_else(|| anyhow::anyhow!("shared metadata names no chain"))?;
    Ok(Some(SharedReadDataset {
        kid,
        content_cid,
        mime_type: metadata
            .pointer("/media/mimeType")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        name: shared_text(metadata, "/name")?,
        rights_policy_identity_base64: rights_policy,
        key_envelope_identity_base64: key_envelope,
        content_key_commitment_base64: content_key_commitment,
        content_identity: decode_runtime_shared_content_identity(&content_identity)?,
        publisher_identity: format!("did:pkh:eip155:{chain_id}:{publisher}"),
    }))
}
