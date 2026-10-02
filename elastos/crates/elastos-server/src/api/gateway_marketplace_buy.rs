//! `buy_offer`: buying one copy of any live market offer (§5.2, §5.3), and
//! adoption -- turning a completed market purchase into the read-side records
//! the unchanged open path consumes (D10).
//!
//! Moved out of `gateway_provider_proxy.rs` unchanged (Q-I2); the purchase
//! stage helpers it shares with the listing-package `buy` stay there.

use super::*;

/// A `buy_offer` answer the page decides on as data rather than by reading a
/// sentence (§5.3). Every one of them is a refusal that raised nothing: no
/// Wallet request, no purchase record.
#[derive(Debug)]
pub(crate) enum RuntimeMarketBuyRefusal {
    /// The offer's price or pay token is not what the buyer agreed to, or it
    /// has no copy left, or it is gone. `current` is the offer as the chain
    /// states it now, or `None` when there is no offer to show.
    TermsChanged { current: Option<serde_json::Value> },
    /// The seller is this Home's own buyer account.
    OwnOffer,
    /// This person already holds the item: minted it, or bought it.
    AlreadyOwned,
    /// The item, its operative, its chain or the asset URI the request names
    /// do not re-derive from the chain and the token's own `tokenURI` (R46:
    /// the KID is not one of them).
    AssetMismatch,
    /// Another purchase of this item is under way and has not been paid for
    /// yet: an attempt recorded on other terms (another seller, price or pay
    /// token -- R28), or a purchase through the other path (R29). `current`
    /// is the recorded attempt's offer, or `None` when the purchase in
    /// progress is not a market offer the page can show.
    AttemptInProgress { current: Option<serde_json::Value> },
}

impl RuntimeMarketBuyRefusal {
    pub(crate) const fn code(&self) -> &'static str {
        match self {
            Self::TermsChanged { .. } => "terms_changed",
            Self::OwnOffer => "own_offer",
            Self::AlreadyOwned => "already_owned",
            Self::AssetMismatch => "asset_mismatch",
            Self::AttemptInProgress { .. } => "attempt_in_progress",
        }
    }

    /// Put this refusal onto a provider error envelope. Its `code` replaces
    /// the generic one, because `code` is the field the page branches on;
    /// `terms_changed` and `attempt_in_progress` also carry the offer the
    /// buyer has to decide on, or `null`.
    pub(crate) fn project_onto(&self, response: &mut serde_json::Value) {
        response["code"] = serde_json::Value::String(self.code().to_string());
        if let Self::TermsChanged { current } | Self::AttemptInProgress { current } = self {
            response["current"] = current.clone().unwrap_or(serde_json::Value::Null);
        }
    }
}

impl std::fmt::Display for RuntimeMarketBuyRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .write_str(crate::protected_content_runtime::RUNTIME_CUSTODY_PURCHASE_DENIED_MESSAGE)
    }
}

impl std::error::Error for RuntimeMarketBuyRefusal {}

fn runtime_market_buy_refused(refusal: RuntimeMarketBuyRefusal, line: u32) -> anyhow::Error {
    tracing::debug!(
        line,
        code = refusal.code(),
        "market purchase refused before any effect"
    );
    anyhow::Error::new(refusal)
}

/// R29 for `buy_offer`: the item is already held, or being bought, through
/// the listing-package path. Complete is `already_owned`; anything else is
/// `attempt_in_progress` with no offer to show -- that purchase is not a
/// market offer.
fn runtime_market_listing_purchase_refusal(
    listing_purchase: &crate::protected_content_runtime::RuntimeCustodyPurchaseRecord,
    line: u32,
) -> anyhow::Error {
    let refusal = if matches!(
        listing_purchase.progress,
        crate::protected_content_runtime::RuntimeCustodyPurchaseProgress::Complete { .. }
    ) {
        RuntimeMarketBuyRefusal::AlreadyOwned
    } else {
        RuntimeMarketBuyRefusal::AttemptInProgress { current: None }
    };
    runtime_market_buy_refused(refusal, line)
}

/// R29 for `buy`: the item is already held, or being bought, as a market
/// purchase. Complete is `already_owned`; anything else is
/// `attempt_in_progress` carrying the market attempt's recorded offer.
pub(super) fn runtime_listing_market_purchase_refusal(
    market_purchase: &crate::protected_content_market::RuntimeMarketPurchaseRecord,
    line: u32,
) -> anyhow::Error {
    let refusal = if matches!(
        market_purchase.progress,
        crate::protected_content_runtime::RuntimeCustodyPurchaseProgress::Complete { .. }
    ) {
        RuntimeMarketBuyRefusal::AlreadyOwned
    } else {
        RuntimeMarketBuyRefusal::AttemptInProgress {
            current: runtime_market_recorded_offer(&market_purchase.offer),
        }
    };
    runtime_market_buy_refused(refusal, line)
}

/// Schema of `buy_offer`'s terminal success answer (R11).
const RUNTIME_MARKET_BUY_COMPLETE_SCHEMA_V1: &str = "elastos.marketplace.buy-offer-complete/v1";

/// `buy_offer`'s terminal success answer (R11): the item bought, the asset it
/// names, and how far adoption got. Never an account address (D14).
/// `item.kid` is `null` while the item's KID is unproven (R46).
fn runtime_market_buy_complete_answer(
    purchase: &crate::protected_content_market::RuntimeMarketPurchaseRecord,
    adoption: &str,
) -> serde_json::Value {
    serde_json::json!({
        "schema": RUNTIME_MARKET_BUY_COMPLETE_SCHEMA_V1,
        "item": {
            "chain_namespace": purchase.item.chain_namespace,
            "network": purchase.item.network,
            "ledger": purchase.item.ledger,
            "token_id": purchase.item.token_id,
            "operative": purchase.item.operative,
            "kid": purchase.item.kid,
        },
        "asset_uri": purchase.asset_uri,
        "adoption": adoption,
    })
}

/// The completed purchase's answer, after adoption has run once more. A KID
/// adoption proved is stated in this same answer: the record is read back,
/// and used when it is still this attempt.
async fn runtime_market_completed_answer(
    state: &GatewayState,
    registry: &Arc<ProviderRegistry>,
    purchase: &crate::protected_content_market::RuntimeMarketPurchaseRecord,
) -> serde_json::Value {
    let adoption = market_purchase_adoption_state(state, registry, purchase).await;
    let current = crate::protected_content_market::load_runtime_market_purchase(
        &state.data_dir,
        &purchase.principal_id,
        &purchase.item,
    )
    .ok()
    .flatten()
    .filter(|current| current.attempt_id == purchase.attempt_id);
    runtime_market_buy_complete_answer(current.as_ref().unwrap_or(purchase), adoption)
}

/// How far adoption has got for a completed market purchase: `adopted`,
/// `foreign` or `pending` (R11).
///
/// Adoption runs here, best effort, every time a completed purchase is
/// answered -- so a buy whose adoption could not finish is retried by the
/// next press. It never fails the purchase: whatever went wrong is logged at
/// `debug` and the answer stays a completed purchase.
async fn market_purchase_adoption_state(
    state: &GatewayState,
    registry: &Arc<ProviderRegistry>,
    purchase: &crate::protected_content_market::RuntimeMarketPurchaseRecord,
) -> &'static str {
    match adopt_runtime_market_purchase(state, registry, &purchase.principal_id, purchase).await {
        Ok(outcome) => {
            if let crate::protected_content_market::AdoptionOutcome::NotYet(reason) = &outcome {
                tracing::debug!(%reason, "market purchase complete; adoption is not possible yet");
            }
            outcome.wire_value()
        }
        Err(error) => {
            tracing::debug!(
                error = %format!("{error:#}"),
                "market purchase complete; adoption failed"
            );
            "pending"
        }
    }
}

/// Read after a buy (D10): turn a completed market purchase into the listing
/// and purchase records the existing open path consumes, from the asset's
/// shared `metadata.json` -- never `manifest.json`.
///
/// - Step 1: the shared document is read; with no ElastOS protection it is
///   `Foreign`. A KID the purchase has not proven is proven here, against
///   the chain's binding, and its access answer read (R46); a document
///   whose KID the chain binds elsewhere is `Mismatch`.
/// - Steps 2-4: the runtime builds the package, verifies the content,
///   computes the mint and records both records
///   (`protected_content_runtime::adopt_runtime_market_listing`).
/// - Step 5: the owned `.ddrm` is filed, embedding that same shared document.
///
/// Anything about the asset itself failing -- the document unreadable, an
/// entry that cannot be adopted, content that does not verify, a listing of
/// the mint on other terms -- is `NotYet(reason)`, and the purchase stays
/// complete. `Err` for a record that names another principal, and for this
/// Home failing to read or write its own records once the read-side records
/// exist (the adopted mint id, the purchase record, the market record's
/// update); the caller answers those as `pending` too, never as a purchase
/// failure.
pub(crate) async fn adopt_runtime_market_purchase(
    state: &GatewayState,
    registry: &Arc<ProviderRegistry>,
    principal_id: &str,
    record: &crate::protected_content_market::RuntimeMarketPurchaseRecord,
) -> anyhow::Result<crate::protected_content_market::AdoptionOutcome> {
    use crate::protected_content_market::AdoptionOutcome;
    if record.principal_id != principal_id {
        anyhow::bail!("Runtime market purchase is invalid");
    }
    if !matches!(
        record.progress,
        crate::protected_content_runtime::RuntimeCustodyPurchaseProgress::Complete { .. }
    ) {
        return Ok(AdoptionOutcome::NotYet(
            "the purchase is not complete".to_string(),
        ));
    }
    // Already adopted, and both records are still here: nothing to redo.
    if let Some(mint_id) = record.adopted_mint_id.as_deref() {
        if let Ok(digest) = crate::protected_content_runtime::parse_mint_id_hex(mint_id) {
            let listing = crate::protected_content_runtime::load_runtime_custody_listing(
                &state.data_dir,
                digest,
            );
            let purchase = crate::protected_content_runtime::load_runtime_custody_purchase(
                &state.data_dir,
                principal_id,
                digest,
            );
            if matches!(listing, Ok(Some(_))) && matches!(purchase, Ok(Some(_))) {
                return Ok(AdoptionOutcome::Adopted {
                    mint_id: mint_id.to_string(),
                });
            }
        }
    }
    let Some(folder) = record.asset_uri.strip_prefix("elastos://") else {
        return Ok(AdoptionOutcome::NotYet(
            "the asset URI is invalid".to_string(),
        ));
    };
    // The same cap, inside the fetch, as the listing's read of this document
    // (R30).
    let metadata = match crate::content::fetch_bytes_via_provider_capped(
        registry.as_ref(),
        folder,
        Some("metadata.json"),
        MARKET_LISTING_DOCUMENT_MAX_BYTES,
    )
    .await
    {
        Ok(bytes) if bytes.len() <= MARKET_LISTING_DOCUMENT_MAX_BYTES => {
            match serde_json::from_slice::<serde_json::Value>(&bytes) {
                Ok(document) if document.is_object() => document,
                _ => {
                    return Ok(AdoptionOutcome::NotYet(
                        "the shared document is not a JSON object".to_string(),
                    ))
                }
            }
        }
        Ok(_) => {
            return Ok(AdoptionOutcome::NotYet(
                "the shared document is too large".to_string(),
            ))
        }
        Err(error) => {
            return Ok(AdoptionOutcome::NotYet(format!(
                "the shared document could not be read: {error:#}"
            )))
        }
    };
    let dataset = crate::protected_content_market::shared_read_dataset(&metadata);
    let foreign = matches!(dataset, Ok(None));
    // R46: a purchase completed with its KID unknown learns it here, from the
    // shared document and the chain's binding, before anything is adopted --
    // whether or not the document can then be adopted. A foreign asset stays
    // foreign whatever the chain could not answer about its KID.
    let record = match classify_metadata_kid(&metadata) {
        MetadataKid::Named(kid) => {
            match prove_runtime_market_purchase_kid(state, record, &kid).await? {
                std::ops::ControlFlow::Continue(proven) => proven,
                std::ops::ControlFlow::Break(AdoptionOutcome::NotYet(_)) if foreign => {
                    return Ok(AdoptionOutcome::Foreign)
                }
                std::ops::ControlFlow::Break(outcome) => return Ok(outcome),
            }
        }
        MetadataKid::Contradictory => {
            tracing::warn!("market purchase not adopted: the shared document names two kids");
            return Ok(AdoptionOutcome::Mismatch);
        }
        MetadataKid::Absent => record.clone(),
    };
    let record = &record;
    let dataset = match dataset {
        Ok(Some(dataset)) => dataset,
        Ok(None) => return Ok(AdoptionOutcome::Foreign),
        Err(error) => return Ok(AdoptionOutcome::NotYet(format!("{error:#}"))),
    };
    let profile_did = match crate::protected_content_runtime::load_runtime_custody_profile_did(
        &state.data_dir,
        principal_id,
    ) {
        Ok(profile_did) => profile_did,
        Err(error) => return Ok(AdoptionOutcome::NotYet(format!("{error:#}"))),
    };
    let listing = match crate::protected_content_runtime::adopt_runtime_market_listing(
        &state.data_dir,
        registry,
        &profile_did,
        record,
        &dataset,
    )
    .await
    {
        Ok(listing) => listing,
        Err(error) => return Ok(AdoptionOutcome::NotYet(format!("{error:#}"))),
    };
    let mint_id = listing.package.mint_id.clone();
    let digest = crate::protected_content_runtime::parse_mint_id_hex(&mint_id)?;
    // Step 5: file the `.ddrm`, once. A copy already filed is not filed again.
    if let Some(mut purchase) = crate::protected_content_runtime::load_runtime_custody_purchase(
        &state.data_dir,
        principal_id,
        digest,
    )? {
        if purchase.capsule_uri.is_none() {
            purchase.capsule_uri = write_runtime_custody_owned_capsule(
                state,
                registry.as_ref(),
                &listing.package,
                principal_id,
                purchase.acquisition,
            )
            .await;
            if purchase.capsule_uri.is_some() {
                purchase.updated_at = crate::auth::now_ts();
                crate::protected_content_runtime::persist_runtime_custody_purchase(
                    &state.data_dir,
                    &purchase,
                )?;
            }
        }
    }
    if record.adopted_mint_id.as_deref() != Some(mint_id.as_str()) {
        let mut adopted = record.clone();
        adopted.adopted_mint_id = Some(mint_id.clone());
        adopted.updated_at = crate::auth::now_ts();
        // Compare-by-attempt (R26), like every other write of this record: a
        // record that is no longer this attempt is left exactly as it is. The
        // read-side records exist either way, so the answer stays `Adopted`;
        // the next answer for the attempt now on disk adopts it afresh.
        if !crate::protected_content_market::update_runtime_market_purchase(
            &state.data_dir,
            &adopted,
        )? {
            tracing::debug!(
                "market purchase adopted; its record is no longer this attempt and was left as is"
            );
        }
    }
    Ok(AdoptionOutcome::Adopted { mint_id })
}

/// R46: make `record`'s KID and access evidence proven facts before adoption.
///
/// A record that already names its KID keeps it; a document naming another
/// KID contradicts what the chain proved, and is `Mismatch`. An unknown KID
/// is taken from the document only once the chain binds it to this very
/// item (`ipReference`): bound elsewhere is `Mismatch`, and a binding the
/// chain cannot state is `NotYet`. Then, if the purchase completed on its
/// receipt alone, the chain's access answer for that KID is read -- the one
/// the open path requires -- and a chain that has not granted it is
/// `NotYet`. What was learned is written back to the market record, only
/// while it is still this attempt (R26). `Break` carries the outcome.
async fn prove_runtime_market_purchase_kid(
    state: &GatewayState,
    record: &crate::protected_content_market::RuntimeMarketPurchaseRecord,
    document_kid: &str,
) -> anyhow::Result<
    std::ops::ControlFlow<
        crate::protected_content_market::AdoptionOutcome,
        crate::protected_content_market::RuntimeMarketPurchaseRecord,
    >,
> {
    use crate::protected_content_market::AdoptionOutcome;
    use std::ops::ControlFlow::{Break, Continue};
    let crate::protected_content_runtime::RuntimeCustodyPurchaseProgress::Complete { terminal } =
        &record.progress
    else {
        return Ok(Break(AdoptionOutcome::NotYet(
            "the purchase is not complete".to_string(),
        )));
    };
    let mut proven = record.clone();
    match record.item.kid.as_deref() {
        Some(kid) if kid == document_kid => {}
        Some(_) => {
            tracing::warn!("market purchase not adopted: the shared document names another kid");
            return Ok(Break(AdoptionOutcome::Mismatch));
        }
        None => match market_kid_binds_to_item(state, &record.item, document_kid).await {
            Ok(true) => proven.item.kid = Some(document_kid.to_string()),
            Ok(false) => {
                tracing::warn!(
                    "market purchase not adopted: the shared document's kid is bound to another item"
                );
                return Ok(Break(AdoptionOutcome::Mismatch));
            }
            Err(refusal) => {
                return Ok(Break(AdoptionOutcome::NotYet(format!(
                    "the kid binding could not be read: {refusal:?}"
                ))))
            }
        },
    }
    // The open path asks for the KID's own answer. Evidence asked by the
    // item (R50), or none at all, is replaced by it once the KID is proven.
    if !terminal.access_evidence.as_ref().is_some_and(|evidence| {
        evidence.has_access && evidence.content_access_id.as_deref() == Some(document_kid)
    }) {
        let buyer_account = RuntimeCustodyCreatorAccount {
            account_id: record.account_id.clone(),
            address: record.address.clone(),
            external_signer: false,
            connector_id: None,
        };
        let access =
            match runtime_market_access_evidence(state, record, Some(document_kid), &buyer_account)
                .await
            {
                Ok(Some(access)) => access,
                Ok(None) => {
                    return Ok(Break(AdoptionOutcome::NotYet(
                        "the chain has not granted access for the kid yet".to_string(),
                    )))
                }
                Err(error) => {
                    return Ok(Break(AdoptionOutcome::NotYet(format!(
                        "the access for the kid could not be read: {error:#}"
                    ))))
                }
            };
        let mut terminal = terminal.clone();
        terminal.access_evidence = Some(access);
        proven.progress =
            crate::protected_content_runtime::RuntimeCustodyPurchaseProgress::Complete { terminal };
    }
    if proven != *record {
        proven.updated_at = crate::auth::now_ts();
        if !crate::protected_content_market::update_runtime_market_purchase(
            &state.data_dir,
            &proven,
        )? {
            return Ok(Break(AdoptionOutcome::NotYet(
                "the market purchase is no longer this attempt".to_string(),
            )));
        }
        tracing::debug!("market purchase: kid proven from the shared document and the chain");
    }
    Ok(Continue(proven))
}

/// An offer in the shape a `ListingObject` offer takes (§5.1, R16):
/// `payment_processor` is always present, `null` for a native-token offer.
/// `None` for an ERC-20 offer that names no processor: the listing refuses to
/// state such an offer, so no answer states it either (R35).
fn runtime_market_offer_shape(
    seller: &str,
    quantity: &str,
    price: &str,
    pay_token: &str,
    payment_processor: Option<&str>,
) -> Option<serde_json::Value> {
    let pay_token = pay_token.to_ascii_lowercase();
    let payment_processor = if pay_token == RUNTIME_CUSTODY_NATIVE_PAY_TOKEN {
        serde_json::Value::Null
    } else {
        serde_json::Value::String(payment_processor?.to_ascii_lowercase())
    };
    Some(serde_json::json!({
        "seller": seller.to_ascii_lowercase(),
        "quantity": quantity,
        "price": price,
        "pay_token": pay_token,
        "payment_processor": payment_processor,
    }))
}

/// The offer as the chain states it now (§5.1, R16, R35).
fn runtime_market_current_offer(
    listing: &ResolvedProtectedContentPurchaseListing,
) -> Option<serde_json::Value> {
    runtime_market_offer_shape(
        &listing.seller,
        &listing.available_quantity,
        &listing.price,
        &listing.pay_token,
        listing.payment_processor.as_deref(),
    )
}

/// A recorded attempt's offer, for `attempt_in_progress` (R28): the terms
/// it records, for the one copy it buys.
fn runtime_market_recorded_offer(
    offer: &crate::protected_content_market::RuntimeMarketOffer,
) -> Option<serde_json::Value> {
    runtime_market_offer_shape(
        &offer.seller,
        &offer.quantity,
        &offer.price,
        &offer.pay_token,
        offer.payment_processor.as_deref(),
    )
}

/// The terms of `listing` as a `RuntimeMarketOffer`, for one copy.
fn runtime_market_offer_from_listing(
    listing: &ResolvedProtectedContentPurchaseListing,
) -> crate::protected_content_market::RuntimeMarketOffer {
    crate::protected_content_market::RuntimeMarketOffer {
        seller: listing.seller.to_ascii_lowercase(),
        price: listing.price.clone(),
        pay_token: listing.pay_token.to_ascii_lowercase(),
        payment_processor: listing
            .payment_processor
            .as_deref()
            .map(str::to_ascii_lowercase),
        quantity: "0x1".to_string(),
    }
}

/// D7: a buy goes ahead only on the terms the buyer agreed to. `None` when
/// the fresh plan sells one copy on exactly those terms; otherwise the
/// `terms_changed` answer, carrying the offer as it stands now -- or `null`
/// when it has no copy left or is gone, exactly as a `ListingObject` drops a
/// sold-out offer.
fn runtime_market_terms_changed(
    agreed: &crate::protected_content_market::RuntimeMarketOffer,
    plan: Option<&ResolvedProtectedContentPurchase>,
) -> Option<RuntimeMarketBuyRefusal> {
    let Some(plan) = plan else {
        return Some(RuntimeMarketBuyRefusal::TermsChanged { current: None });
    };
    let listing = &plan.verified_listing;
    if listing.available_quantity == "0x0" {
        return Some(RuntimeMarketBuyRefusal::TermsChanged { current: None });
    }
    if !agreed.same_terms_as(&runtime_market_offer_from_listing(listing)) {
        return Some(RuntimeMarketBuyRefusal::TermsChanged {
            current: runtime_market_current_offer(listing),
        });
    }
    None
}

/// Step 1: the request is well formed, and the terms the buyer agreed to as a
/// `RuntimeMarketOffer` (seller from the top of the request; no processor --
/// the page never names one, and it is not a term). Nothing here is trusted
/// yet; this only refuses a request no binding check could make sense of.
fn validate_runtime_market_buy_request(
    input: &crate::protected_content_market::RuntimeMarketBuyInput,
) -> anyhow::Result<crate::protected_content_market::RuntimeMarketOffer> {
    let item = &input.item;
    let text_ok = |value: &str| {
        !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
    };
    if !text_ok(&item.chain_namespace) || !text_ok(&item.network) {
        return Err(purchase_denied_missing!()());
    }
    crate::protected_content_runtime::validate_runtime_custody_evm_address(&item.ledger)
        .map_err(purchase_denied!())?;
    crate::protected_content_runtime::validate_runtime_custody_canonical_quantity(&item.token_id)
        .map_err(purchase_denied!())?;
    // `item.kid` gates nothing (R46): the purchase rests on the chain's terms
    // alone, and a KID is proven only by adoption, after payment.
    let agreed = crate::protected_content_market::RuntimeMarketOffer {
        seller: input.seller.clone(),
        price: input.agreed.price.clone(),
        pay_token: input.agreed.pay_token.clone(),
        payment_processor: None,
        quantity: input.agreed.quantity.clone(),
    };
    agreed.validate().map_err(purchase_denied!())?;
    if agreed.quantity != "0x1" {
        return Err(purchase_denied_missing!()());
    }
    Ok(agreed)
}

/// Step 2: re-derive the item from the chain alone (R46) -- the market
/// source's chain, `(ledger, token_id)`'s non-zero operative and the asset
/// folder its own `tokenURI` names -- and refuse anything the request
/// asserted that this does not re-derive. Neither the KID binding nor any
/// document is read, and a KID the page sent is not a precondition: a
/// purchase rests on the chain's terms, never on content availability or a
/// proof that its key can be recovered. Returns the verified item, its KID
/// unknown, and its asset URI -- the only item identity the rest of the
/// purchase uses.
async fn verify_runtime_market_buy_item(
    state: &GatewayState,
    input: &crate::protected_content_market::RuntimeMarketBuyInput,
) -> anyhow::Result<(crate::protected_content_market::RuntimeMarketItem, String)> {
    let verified =
        match verify_market_item_on_chain(state, &input.item.ledger, &input.item.token_id).await {
            Ok(verified) => verified,
            Err(MarketListingRefusal::AssetMismatch | MarketListingRefusal::LegacyListing) => {
                return Err(runtime_market_buy_refused(
                    RuntimeMarketBuyRefusal::AssetMismatch,
                    line!(),
                ))
            }
            Err(MarketListingRefusal::Unbound) => {
                tracing::warn!("market purchase target unbound on chain");
                anyhow::bail!(
                    crate::protected_content_runtime::RUNTIME_CUSTODY_PURCHASE_UNBOUND_MESSAGE
                );
            }
            Err(MarketListingRefusal::Unavailable(reason)) => {
                return Err(purchase_unavailable!()(reason))
            }
        };
    let asset_uri = format!("elastos://{}", verified.folder_cid);
    // The operative is never a claim (R20): it is the one the chain names.
    let mismatch = if input.asset_uri != asset_uri {
        Some("asset_uri")
    } else if input.item.chain_namespace != verified.item.chain_namespace
        || input.item.network != verified.item.network
    {
        Some("chain")
    } else {
        None
    };
    if let Some(field) = mismatch {
        tracing::debug!(
            field,
            "market purchase refused: the request's item does not bind"
        );
        return Err(runtime_market_buy_refused(
            RuntimeMarketBuyRefusal::AssetMismatch,
            line!(),
        ));
    }
    Ok((verified.item, asset_uri))
}

/// Whether this person already holds `item` -- a listing of theirs that says
/// `creator` or `purchased`, or the chain granting their buyer account
/// access: by the KID when it is proven, by the item when it is not (R50) --
/// so a second press never pays for what they own, here or through another
/// Home. A chain that cannot answer fails closed: no purchase starts on an
/// unknown.
async fn runtime_market_item_already_owned(
    state: &GatewayState,
    principal_id: &str,
    item: &crate::protected_content_market::RuntimeMarketItem,
    buyer_account: &RuntimeCustodyCreatorAccount,
) -> anyhow::Result<bool> {
    let local = crate::protected_content_runtime::runtime_custody_listing_chain_index(
        &state.data_dir,
        principal_id,
    )
    .map_err(purchase_unavailable!())?;
    if local.iter().any(|entry| {
        entry.chain_namespace == item.chain_namespace
            && entry.ledger.eq_ignore_ascii_case(&item.ledger)
            && entry.token_id.eq_ignore_ascii_case(&item.token_id)
            && matches!(entry.access_state.as_str(), "creator" | "purchased")
    }) {
        return Ok(true);
    }
    let request_id = format!("market-purchase-owned:{}:{}", item.ledger, item.token_id);
    let access = match item.kid.as_deref() {
        Some(kid) => {
            resolve_runtime_custody_item_access(
                state,
                &item.chain_namespace,
                &item.network,
                kid,
                buyer_account,
                &request_id,
            )
            .await
        }
        // R50: no KID, so the chain is asked by the item itself. An item
        // owned through another Home or ela.city is refused here, before
        // anything is raised.
        None => resolve_runtime_market_item_access(state, item, buyer_account, &request_id).await,
    }
    .map_err(purchase_unavailable!())?;
    Ok(access.is_some())
}

/// Fallback signal only: an error whose chain names a revert (the node keeps
/// its own verdict in the message). The mined receipt's `status` is the
/// authoritative signal; a revert known only from these words proves nothing
/// is spent, so its attempt is kept and the answer is the plain failure (M1).
fn runtime_market_buy_error_is_revert(error: &anyhow::Error) -> bool {
    format!("{error:#}").to_ascii_lowercase().contains("revert")
}

/// A receipt -- as a completion carries it, or as the `receipt` op answers --
/// whose `status` says the transaction was mined and reverted.
pub(super) fn runtime_market_receipt_reverted(receipt: Option<&serde_json::Value>) -> bool {
    receipt
        .and_then(|receipt| receipt.pointer("/receipt/status"))
        .and_then(serde_json::Value::as_str)
        == Some("0x0")
}

/// What the chain's receipt says about a confirmed buy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeMarketBuyReceipt {
    /// Mined, `status` 0x1: the buy landed.
    Succeeded,
    /// Mined, `status` 0x0: the buy reverted.
    Reverted,
    /// Absent, unreadable, or not this transaction's.
    Unknown,
}

/// `receipt`'s verdict on transaction `hash` on `network`, when it is a
/// receipt of that transaction; `Unknown` otherwise.
fn runtime_market_receipt_verdict(
    receipt: &serde_json::Value,
    network: &str,
    hash: &str,
) -> RuntimeMarketBuyReceipt {
    let about_this_buy = receipt.get("network").and_then(serde_json::Value::as_str)
        == Some(network)
        && receipt
            .pointer("/receipt/transactionHash")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|observed| observed.eq_ignore_ascii_case(hash));
    match receipt
        .pointer("/receipt/status")
        .and_then(serde_json::Value::as_str)
    {
        Some("0x1") if about_this_buy => RuntimeMarketBuyReceipt::Succeeded,
        Some("0x0") if about_this_buy => RuntimeMarketBuyReceipt::Reverted,
        _ => RuntimeMarketBuyReceipt::Unknown,
    }
}

/// The confirmed buy `hash`'s receipt: the one the Wallet observed when the
/// buy confirmed (`observed`), or else the chain's own answer now. Anything
/// the chain cannot answer, or a receipt that is absent or not for this
/// transaction, is `Unknown`: the attempt keeps waiting, never decided on a
/// guess.
async fn runtime_market_buy_receipt(
    state: &GatewayState,
    network: &str,
    hash: &str,
    observed: Option<&serde_json::Value>,
) -> RuntimeMarketBuyReceipt {
    if let Some(verdict) = observed
        .map(|receipt| runtime_market_receipt_verdict(receipt, network, hash))
        .filter(|verdict| *verdict != RuntimeMarketBuyReceipt::Unknown)
    {
        return verdict;
    }
    // Reads the latest block, not a finalized one. Acceptable: a reorg that
    // un-reverts this buy leaves the item owned, and the next attempt's
    // already_owned check refuses to buy it again.
    match wallet_chain_provider_data(
        state,
        serde_json::json!({ "op": "receipt", "network": network, "hash": hash }),
    )
    .await
    {
        Ok(answer) => runtime_market_receipt_verdict(&answer, network, hash),
        Err((status, message)) => {
            tracing::debug!(
                status = status.as_u16(),
                error = %message,
                "market purchase: the buy receipt could not be read"
            );
            RuntimeMarketBuyReceipt::Unknown
        }
    }
}

/// Whether the confirmed buy transaction `hash` was mined and reverted, as
/// the chain's own receipt says. Anything the chain cannot answer, or a
/// receipt that is absent or not for this transaction, is "not known to have
/// reverted": the attempt keeps waiting, never retired on a guess.
pub(super) async fn runtime_market_buy_mined_reverted(
    state: &GatewayState,
    network: &str,
    hash: &str,
) -> bool {
    runtime_market_buy_receipt(state, network, hash, None).await
        == RuntimeMarketBuyReceipt::Reverted
}

/// A driver whose attempt is no longer the recorded one -- retired, or
/// superseded by a newer attempt -- stops and leaves the record alone (R26).
/// The answer is a wait: the next press re-reads whatever is recorded.
fn runtime_market_attempt_superseded(
    stage: crate::protected_content_runtime::RuntimeCustodyBuyStage,
    line: u32,
) -> anyhow::Error {
    tracing::debug!(
        line,
        "market purchase: this attempt is no longer the recorded one"
    );
    runtime_custody_buy_progress(
        crate::protected_content_runtime::RuntimeCustodyBuyProgress::waiting(stage),
        line,
    )
}

/// Retire `purchase`'s attempt if the record is still that attempt (R26).
/// `Ok(false)`: another attempt holds the record, and was left untouched.
fn retire_runtime_market_attempt(
    state: &GatewayState,
    purchase: &crate::protected_content_market::RuntimeMarketPurchaseRecord,
) -> anyhow::Result<bool> {
    crate::protected_content_market::retire_runtime_market_purchase(
        &state.data_dir,
        &purchase.principal_id,
        &purchase.item,
        &purchase.attempt_id,
    )
    .map_err(purchase_unavailable!())
}

/// Step 8: a buy the chain mined and reverted. Its effect is spent, so the
/// attempt is retired first (R21, only if still this attempt -- R26) and the
/// buyer's next press starts afresh. Then the offer is read again: terms that
/// moved are `terms_changed` -- the likeliest reason, since the contract
/// refuses a buy at terms other than the live listing's -- and anything else
/// keeps `failure`, today's answer.
async fn runtime_market_buy_reverted(
    state: &GatewayState,
    purchase: &crate::protected_content_market::RuntimeMarketPurchaseRecord,
    failure: anyhow::Error,
) -> anyhow::Error {
    tracing::warn!(
        effect_id = %purchase.acquisition_stage.effect_id,
        "market purchase: the buy transaction was mined and reverted"
    );
    match retire_runtime_market_attempt(state, purchase) {
        Ok(true) => {}
        Ok(false) => {
            return runtime_market_attempt_superseded(
                crate::protected_content_runtime::RuntimeCustodyBuyStage::ChainSettlement,
                line!(),
            )
        }
        // A record that could not be retired is still a dead attempt; say so
        // rather than answer as if the next press could start afresh.
        Err(error) => return error,
    }
    match read_runtime_custody_purchase_plan(state, &purchase.item, &purchase.offer.seller).await {
        Ok(plan) => match runtime_market_terms_changed(&purchase.offer, plan.as_ref()) {
            Some(refusal) => runtime_market_buy_refused(refusal, line!()),
            None => failure,
        },
        Err(error) => {
            tracing::debug!(
                error = %format!("{error:#}"),
                "market purchase: the offer could not be re-read after a revert"
            );
            failure
        }
    }
}

/// Resume a recorded attempt on its own terms, or answer it if complete.
/// `Break` carries the completed answer.
///
/// R28: a press resumes the recorded attempt only when it names the terms
/// that attempt records (seller, price, pay token). A press on other terms,
/// while the recorded attempt has no confirmed buy, is answered
/// `attempt_in_progress` with the recorded offer, and drives nothing: the page
/// then shows which purchase is under way instead of showing another seller
/// busy while the Wallet re-raises the first. Once a buy is confirmed, the
/// money has moved and the attempt is finished whatever the press names.
async fn resume_runtime_market_attempt(
    state: &GatewayState,
    registry: &Arc<ProviderRegistry>,
    existing: crate::protected_content_market::RuntimeMarketPurchaseRecord,
    item: &crate::protected_content_market::RuntimeMarketItem,
    asset_uri: &str,
    agreed: &crate::protected_content_market::RuntimeMarketOffer,
    resolved_buyer: &RuntimeCustodyCreatorAccount,
) -> anyhow::Result<
    std::ops::ControlFlow<
        serde_json::Value,
        (
            crate::protected_content_market::RuntimeMarketPurchaseRecord,
            RuntimeCustodyCreatorAccount,
        ),
    >,
> {
    // The recorded item may know its KID (recorded before R46, or learned by
    // adoption); the item this press verified never does. Both are the same
    // item on the same operative.
    if !existing.item.same_item_as(item) || existing.asset_uri != asset_uri {
        return Err(purchase_denied_missing!()());
    }
    validate_wallet_evm_address(&existing.address, "buyer").map_err(purchase_denied!())?;
    if matches!(
        existing.progress,
        crate::protected_content_runtime::RuntimeCustodyPurchaseProgress::Complete { .. }
    ) {
        return Ok(std::ops::ControlFlow::Break(
            runtime_market_completed_answer(state, registry, &existing).await,
        ));
    }
    let buy_confirmed = matches!(
        existing.progress,
        crate::protected_content_runtime::RuntimeCustodyPurchaseProgress::Pending {
            confirmed_buy: Some(_),
            ..
        }
    );
    if !buy_confirmed && !existing.offer.same_terms_as(agreed) {
        return Err(runtime_market_buy_refused(
            RuntimeMarketBuyRefusal::AttemptInProgress {
                current: runtime_market_recorded_offer(&existing.offer),
            },
            line!(),
        ));
    }
    // The attempt resumes on the terms it recorded when it started: those are
    // the terms its Wallet request already carries, and a second press must
    // never pay twice or pay differently.
    tracing::debug!(
        effect_id = %existing.acquisition_stage.effect_id,
        "market purchase: resuming the recorded attempt on its recorded terms"
    );
    let buyer_account = if existing.account_id == resolved_buyer.account_id
        && existing
            .address
            .eq_ignore_ascii_case(&resolved_buyer.address)
    {
        resolved_buyer.clone()
    } else {
        // Rebuilt from the record, which stores the identity and not the
        // wallet surface it came from; the signer fields stay at the value
        // that claims the least, as the listing path does.
        RuntimeCustodyCreatorAccount {
            account_id: existing.account_id.clone(),
            address: existing.address.clone(),
            external_signer: false,
            connector_id: None,
        }
    };
    Ok(std::ops::ControlFlow::Continue((existing, buyer_account)))
}

/// `buy_offer`: buy one copy of any live offer, on the terms the buyer saw.
///
/// D1 -- buy and read are separate workflows. This function deliberately
/// never calls `verify_fresh_runtime_custody_availability`, and runs no
/// availability receipt, content verification, draft rebuild or
/// `manifest.json` read. Those answer "can I open it", which is the open
/// path's question; the access token this buys exists on chain whether or
/// not any file is retrievable, so nothing about the content decides a buy.
pub(crate) async fn runtime_custody_buy_offer_via_gateway(
    state: &GatewayState,
    authority: &RuntimeWalletAuthority,
    registry: Arc<ProviderRegistry>,
    input: crate::protected_content_market::RuntimeMarketBuyInput,
) -> anyhow::Result<serde_json::Value> {
    let agreed = validate_runtime_market_buy_request(&input)?;
    let (item, asset_uri) = verify_runtime_market_buy_item(state, &input).await?;
    let resolved_buyer =
        resolve_runtime_custody_buyer_account(state, authority, &item.chain_namespace).await?;
    let existing = crate::protected_content_market::load_runtime_market_purchase(
        &state.data_dir,
        &input.principal_id,
        &item,
    )?;
    let (purchase, buyer_account) = match existing {
        Some(existing) => match resume_runtime_market_attempt(
            state,
            &registry,
            existing,
            &item,
            &asset_uri,
            &agreed,
            &resolved_buyer,
        )
        .await?
        {
            std::ops::ControlFlow::Break(answer) => return Ok(answer),
            std::ops::ControlFlow::Continue(resumed) => resumed,
        },
        None => {
            let buyer_account = resolved_buyer.clone();
            // R29, early: the listing-package path already holds this item.
            // Checked again, under the lock, when the attempt is recorded.
            if let Some(listing_purchase) =
                crate::protected_content_runtime::load_runtime_custody_purchase_for_chain_item(
                    &state.data_dir,
                    &input.principal_id,
                    &item.chain_namespace,
                    &item.ledger,
                    &item.token_id,
                )
                .map_err(purchase_unavailable!())?
            {
                return Err(runtime_market_listing_purchase_refusal(
                    &listing_purchase,
                    line!(),
                ));
            }
            if runtime_market_item_already_owned(state, &input.principal_id, &item, &buyer_account)
                .await?
            {
                return Err(runtime_market_buy_refused(
                    RuntimeMarketBuyRefusal::AlreadyOwned,
                    line!(),
                ));
            }
            let plan = read_runtime_custody_purchase_plan(state, &item, &agreed.seller).await?;
            if let Some(refusal) = runtime_market_terms_changed(&agreed, plan.as_ref()) {
                return Err(runtime_market_buy_refused(refusal, line!()));
            }
            let plan = plan.ok_or_else(purchase_unavailable_missing!())?;
            // R33: only now is the seller known to hold a live offer, so only
            // now is "this is your own offer" an answer about that offer --
            // never a way for the page to test whether an arbitrary address is
            // this Home's account.
            if agreed.seller.eq_ignore_ascii_case(&resolved_buyer.address) {
                return Err(runtime_market_buy_refused(
                    RuntimeMarketBuyRefusal::OwnOffer,
                    line!(),
                ));
            }
            // The agreed terms, as the chain spells them: equal to what the
            // buyer agreed to (checked just above), canonical, and carrying
            // the processor an ERC-20 offer pays through.
            let offer = runtime_market_offer_from_listing(&plan.verified_listing);
            // A new attempt draws its own identity (R23); resuming reuses the
            // recorded one, so only a genuinely new attempt is a new effect.
            let attempt_id = crate::protected_content_market::new_runtime_market_attempt_id()
                .map_err(purchase_unavailable!())?;
            let binding = RuntimePurchaseBinding::Market {
                asset_uri: &asset_uri,
                attempt_id: &attempt_id,
            };
            let approval_request = match plan.steps.as_slice() {
                [approval, buy] if approval.stage == "approval" && buy.stage == "buy" => {
                    Some(runtime_custody_purchase_transaction_request(
                        &input.principal_id,
                        &buyer_account,
                        &item,
                        &offer,
                        &binding,
                        approval,
                    )?)
                }
                [buy] if buy.stage == "buy" => None,
                _ => return Err(purchase_unavailable_missing!()()),
            };
            let buy_step = plan
                .steps
                .last()
                .ok_or_else(purchase_unavailable_missing!())?;
            let buy_request = runtime_custody_purchase_transaction_request(
                &input.principal_id,
                &buyer_account,
                &item,
                &offer,
                &binding,
                buy_step,
            )?;
            let now = crate::auth::now_ts();
            let purchase = crate::protected_content_market::RuntimeMarketPurchaseRecord {
                schema: crate::protected_content_market::RUNTIME_MARKET_PURCHASE_SCHEMA_V1
                    .to_string(),
                principal_id: input.principal_id.clone(),
                account_id: buyer_account.account_id.clone(),
                address: buyer_account.address.clone(),
                item: item.clone(),
                asset_uri: asset_uri.clone(),
                offer,
                attempt_id: attempt_id.clone(),
                approval_stage: approval_request
                    .as_ref()
                    .map(|request| runtime_custody_purchase_stage_record("approval", request))
                    .transpose()?,
                acquisition_stage: runtime_custody_purchase_stage_record("buy", &buy_request)?,
                progress:
                    crate::protected_content_runtime::RuntimeCustodyPurchaseProgress::Pending {
                        confirmed_approval: None,
                        confirmed_buy: None,
                    },
                adopted_mint_id: None,
                created_at: now,
                updated_at: now,
            };
            // Recorded before the first Wallet effect is raised, so a reload
            // or a second press finds this attempt and resumes it -- and
            // create-only (R26): a press that loses the race to record an
            // attempt resumes the one that won, so two presses are one effect.
            match crate::protected_content_market::create_runtime_market_purchase(
                &state.data_dir,
                &purchase,
            )
            .map_err(purchase_unavailable!())?
            {
                crate::protected_content_market::RuntimeMarketCreateOutcome::Created => {
                    (purchase, buyer_account)
                }
                crate::protected_content_market::RuntimeMarketCreateOutcome::Existing(winner) => {
                    tracing::debug!("market purchase: another press recorded this attempt first");
                    match resume_runtime_market_attempt(
                        state,
                        &registry,
                        *winner,
                        &item,
                        &asset_uri,
                        &agreed,
                        &resolved_buyer,
                    )
                    .await?
                    {
                        std::ops::ControlFlow::Break(answer) => return Ok(answer),
                        std::ops::ControlFlow::Continue(resumed) => resumed,
                    }
                }
                crate::protected_content_market::RuntimeMarketCreateOutcome::ListingPurchase(
                    listing_purchase,
                ) => {
                    return Err(runtime_market_listing_purchase_refusal(
                        &listing_purchase,
                        line!(),
                    ))
                }
            }
        }
    };
    drive_runtime_market_purchase(state, authority, registry, purchase, buyer_account).await
}

/// Access evidence: from `hasAccessByContentId` asked with the item's proven
/// KID -- never one a request named -- or, with no KID proven, from
/// `hasAccess` asked by the item itself (R50). `None` while the chain has not
/// granted it yet, or cannot answer yet.
async fn runtime_market_access_evidence(
    state: &GatewayState,
    purchase: &crate::protected_content_market::RuntimeMarketPurchaseRecord,
    kid: Option<&str>,
    buyer_account: &RuntimeCustodyCreatorAccount,
) -> anyhow::Result<Option<ResolvedProtectedContentPurchaseAccess>> {
    let request_id = format!(
        "market-purchase-access:{}",
        purchase.acquisition_stage.effect_id
    );
    let access = match kid {
        Some(kid) => {
            resolve_runtime_custody_item_access(
                state,
                &purchase.item.chain_namespace,
                &purchase.item.network,
                kid,
                buyer_account,
                &request_id,
            )
            .await
        }
        None => {
            resolve_runtime_market_item_access(state, &purchase.item, buyer_account, &request_id)
                .await
        }
    };
    match access {
        Ok(access) => Ok(access),
        Err(error) => match error.downcast_ref::<RuntimeCustodyAccessUnanswered>() {
            Some(unanswered) => {
                tracing::debug!(
                    %request_id,
                    status = unanswered.status.as_u16(),
                    error = %unanswered.message,
                    "market purchase: access evidence not readable yet"
                );
                Ok(None)
            }
            None => Err(error),
        },
    }
}

/// A confirmed buy whose landing the chain does not state yet. A buy the
/// chain mined and reverted never lands: it is retired (R21). Otherwise the
/// attempt waits, recorded only while still this attempt, and the same buy
/// completes as soon as the chain says so -- nobody need act.
async fn runtime_market_await_buy_evidence(
    state: &GatewayState,
    purchase: &crate::protected_content_market::RuntimeMarketPurchaseRecord,
    confirmed_buy: &crate::protected_content_runtime::RuntimeCustodyConfirmedPurchaseStage,
    persist: &impl Fn(
        &mut crate::protected_content_market::RuntimeMarketPurchaseRecord,
    ) -> anyhow::Result<bool>,
) -> anyhow::Error {
    if runtime_market_buy_mined_reverted(
        state,
        &purchase.item.network,
        &confirmed_buy.chain_transaction,
    )
    .await
    {
        let failure = purchase_unavailable_missing!()();
        return runtime_market_buy_reverted(state, purchase, failure).await;
    }
    if let Err(error) = persist(&mut purchase.clone()) {
        return error;
    }
    runtime_custody_buy_progress(
        crate::protected_content_runtime::RuntimeCustodyBuyProgress::waiting(
            crate::protected_content_runtime::RuntimeCustodyBuyStage::AccessEvidence,
        ),
        line!(),
    )
}

/// Steps 6-9 for a recorded market purchase: the same stages, in the same
/// order and with the same stage helpers, as `runtime_custody_buy_via_gateway`
/// drives for a listing-package purchase -- persisted to the market record.
async fn drive_runtime_market_purchase(
    state: &GatewayState,
    authority: &RuntimeWalletAuthority,
    registry: Arc<ProviderRegistry>,
    mut purchase: crate::protected_content_market::RuntimeMarketPurchaseRecord,
    buyer_account: RuntimeCustodyCreatorAccount,
) -> anyhow::Result<serde_json::Value> {
    // Every write is conditional on this attempt still being the recorded
    // one (R26): `Ok(false)` means it was retired or superseded, and the
    // driver stops without touching the record.
    let persist = |purchase: &mut crate::protected_content_market::RuntimeMarketPurchaseRecord| {
        purchase.updated_at = crate::auth::now_ts();
        crate::protected_content_market::update_runtime_market_purchase(&state.data_dir, purchase)
    };
    let asset_uri = purchase.asset_uri.clone();
    let attempt_id = purchase.attempt_id.clone();
    let binding = RuntimePurchaseBinding::Market {
        asset_uri: &asset_uri,
        attempt_id: &attempt_id,
    };
    let approval_request = purchase
        .approval_stage
        .as_ref()
        .map(|stage| {
            validate_runtime_custody_purchase_stage_request(
                &purchase.principal_id,
                &buyer_account,
                &purchase.item,
                &purchase.offer,
                &binding,
                stage,
                "approval",
            )
        })
        .transpose()?;
    let buy_request = validate_runtime_custody_purchase_stage_request(
        &purchase.principal_id,
        &buyer_account,
        &purchase.item,
        &purchase.offer,
        &binding,
        &purchase.acquisition_stage,
        "buy",
    )?;

    if let crate::protected_content_runtime::RuntimeCustodyPurchaseProgress::Complete { .. } =
        purchase.progress
    {
        return Ok(runtime_market_completed_answer(state, &registry, &purchase).await);
    }

    let (confirmed_approval_stage, _) = pending_stages(&purchase.progress);
    if let (Some(approval_request), None) =
        (approval_request.as_ref(), confirmed_approval_stage.as_ref())
    {
        let approval_completion =
            complete_runtime_custody_purchase_stage(state, authority, approval_request).await?;
        let approval_completion = match approval_completion {
            RuntimeCustodyPurchaseStageOutcome::Confirmed(completion) => *completion,
            outcome => {
                // A declined effect is never re-raised or broadcast: the
                // attempt is dead and retired (R25). Any other outcome is a
                // wait, whether or not this attempt is still recorded.
                if matches!(outcome, RuntimeCustodyPurchaseStageOutcome::Declined) {
                    retire_runtime_market_attempt(state, &purchase)?;
                } else {
                    persist(&mut purchase)?;
                }
                return Err(runtime_custody_buy_stage_answer(
                    outcome,
                    crate::protected_content_runtime::RuntimeCustodyBuyStage::AllowanceApproval,
                    &buyer_account,
                    line!(),
                ));
            }
        };
        let Some(confirmed_approval) = confirmed_stage(approval_completion) else {
            return Err(purchase_unavailable_missing!()());
        };
        let (_, confirmed_buy) = pending_stages(&purchase.progress);
        purchase.progress =
            crate::protected_content_runtime::RuntimeCustodyPurchaseProgress::Pending {
                confirmed_approval: Some(confirmed_approval),
                confirmed_buy,
            };
        if !persist(&mut purchase)? {
            return Err(runtime_market_attempt_superseded(
                crate::protected_content_runtime::RuntimeCustodyBuyStage::PurchaseApproval,
                line!(),
            ));
        }
    }

    if let crate::protected_content_runtime::RuntimeCustodyPurchaseProgress::Pending {
        confirmed_buy: None,
        ..
    } = &purchase.progress
    {
        let buy_completion =
            match complete_runtime_custody_purchase_stage(state, authority, &buy_request).await {
                Ok(outcome) => outcome,
                Err(error) if runtime_market_buy_error_is_revert(&error) => {
                    // Fallback signal only: nothing proves the effect is
                    // spent, so the attempt is kept and the answer is the
                    // plain failure -- never `terms_changed`, which would
                    // invite new terms the kept attempt cannot take (M1).
                    tracing::warn!(
                        effect_id = %purchase.acquisition_stage.effect_id,
                        "market purchase: the node refused the buy as a revert; attempt kept"
                    );
                    return Err(error);
                }
                Err(error) => return Err(error),
            };
        let buy_completion = match buy_completion {
            RuntimeCustodyPurchaseStageOutcome::Confirmed(completion) => *completion,
            outcome => {
                if matches!(outcome, RuntimeCustodyPurchaseStageOutcome::Declined) {
                    retire_runtime_market_attempt(state, &purchase)?;
                } else {
                    persist(&mut purchase)?;
                }
                return Err(runtime_custody_buy_stage_answer(
                    outcome,
                    crate::protected_content_runtime::RuntimeCustodyBuyStage::PurchaseApproval,
                    &buyer_account,
                    line!(),
                ));
            }
        };
        if runtime_market_receipt_reverted(buy_completion.receipt.as_ref()) {
            let failure = purchase_unavailable_missing!()();
            return Err(runtime_market_buy_reverted(state, &purchase, failure).await);
        }
        let Some(confirmed_buy) = confirmed_stage(buy_completion) else {
            return Err(purchase_unavailable_missing!()());
        };
        let (confirmed_approval, _) = pending_stages(&purchase.progress);
        purchase.progress =
            crate::protected_content_runtime::RuntimeCustodyPurchaseProgress::Pending {
                confirmed_approval,
                confirmed_buy: Some(confirmed_buy),
            };
        if !persist(&mut purchase)? {
            return Err(runtime_market_attempt_superseded(
                crate::protected_content_runtime::RuntimeCustodyBuyStage::ChainSettlement,
                line!(),
            ));
        }
    }

    let confirmed_buy = match &purchase.progress {
        crate::protected_content_runtime::RuntimeCustodyPurchaseProgress::Pending {
            confirmed_buy: Some(confirmed_buy),
            ..
        } => confirmed_buy.clone(),
        _ => return Err(purchase_unavailable_missing!()()),
    };
    // Step 7: the purchase is complete once the chain grants the buyer
    // access -- asked by the KID when one is proven, by the item otherwise
    // (R50). The mined receipt alone is never the evidence.
    let access_evidence = match runtime_market_access_evidence(
        state,
        &purchase,
        purchase.item.kid.as_deref(),
        &buyer_account,
    )
    .await?
    {
        Some(access) => Some(access),
        None => {
            return Err(runtime_market_await_buy_evidence(
                state,
                &purchase,
                &confirmed_buy,
                &persist,
            )
            .await)
        }
    };
    purchase.progress =
        crate::protected_content_runtime::RuntimeCustodyPurchaseProgress::Complete {
            terminal: crate::protected_content_runtime::RuntimeCustodyTerminalPurchaseRecord {
                chain_transaction: confirmed_buy.chain_transaction,
                wallet_binding: confirmed_buy.wallet_binding,
                chain_observation: confirmed_buy.chain_observation,
                access_evidence,
                confirmed_at: confirmed_buy.confirmed_at,
                acquired_at: crate::auth::now_ts(),
            },
        };
    if !persist(&mut purchase)? {
        return Err(runtime_market_attempt_superseded(
            crate::protected_content_runtime::RuntimeCustodyBuyStage::AccessEvidence,
            line!(),
        ));
    }
    Ok(runtime_market_completed_answer(state, &registry, &purchase).await)
}
