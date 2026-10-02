use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use super::{
    create_runtime_listing_purchase_unless_market, create_runtime_market_purchase,
    load_runtime_market_purchase, market_terms_from_package, persist_runtime_market_purchase,
    retire_runtime_listing_purchase, runtime_market_purchase_lock_path,
    runtime_market_purchase_lock_test_overlap_detected, runtime_market_purchase_path,
    write_owner_only_bytes, RuntimeMarketCreateOutcome, RuntimeMarketItem, RuntimeMarketOffer,
    RuntimeMarketPurchaseRecord, RUNTIME_MARKET_PURCHASE_SCHEMA_V1,
};
use crate::protected_content_runtime::{
    RuntimeCustodyPurchaseProgress, RuntimeCustodyPurchaseStageRecord,
    RuntimePortableListingPackage,
};

#[cfg(unix)]
fn owner_only_dir(path: &Path) {
    fs::create_dir_all(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

#[cfg(not(unix))]
fn owner_only_dir(path: &Path) {
    fs::create_dir_all(path).unwrap();
}

// `ledger` and `seller` deliberately carry a hex letter (rather than an
// all-digit address) so `to_ascii_uppercase()` in
// `market_item_refuses_uppercase_and_short_addresses` /
// `market_offer_refuses_an_uppercase_seller` actually corrupts the value --
// an all-digit address survives `to_ascii_uppercase().replacen("0X", "0x",
// 1)` byte-for-byte (its only ASCII letter is the "0x" prefix, which that
// replace restores), which would make those guard tests vacuous.
fn sample_item() -> RuntimeMarketItem {
    RuntimeMarketItem {
        chain_namespace: "eip155:8453".to_string(),
        network: "base".to_string(),
        ledger: "0x00000000000000000000000000000000000000a2".to_string(),
        token_id: "0x3".to_string(),
        operative: "0x0000000000000000000000000000000000000033".to_string(),
        kid: Some(format!("0x{}", "11".repeat(16))),
    }
}

fn sample_offer() -> RuntimeMarketOffer {
    RuntimeMarketOffer {
        seller: "0x00000000000000000000000000000000000000a4".to_string(),
        price: "0x5".to_string(),
        pay_token: "0x0000000000000000000000000000000000000055".to_string(),
        payment_processor: None,
        quantity: "0x1".to_string(),
    }
}

// --- Task 3: the buy dataset -----------------------------------------------

#[test]
fn market_item_refuses_uppercase_and_short_addresses() {
    let mut item = sample_item();
    item.ledger = item.ledger.to_ascii_uppercase().replacen("0X", "0x", 1);
    assert!(item.validate().is_err());
    let mut item = sample_item();
    item.kid = Some("0x1234".into());
    assert!(item.validate().is_err());
}

#[test]
fn market_item_accepts_a_well_formed_item() {
    assert!(sample_item().validate().is_ok());
}

#[test]
fn market_offer_terms_compare_seller_price_and_token_only() {
    let agreed = sample_offer();
    let mut fresh = sample_offer();
    fresh.quantity = "0x9".into(); // availability is not a term
    assert!(agreed.same_terms_as(&fresh));
    fresh.price = "0x6".into();
    assert!(!agreed.same_terms_as(&fresh));
}

#[test]
fn market_offer_accepts_well_formed_terms() {
    assert!(sample_offer().validate().is_ok());
}

#[test]
fn market_offer_refuses_an_uppercase_seller() {
    let mut offer = sample_offer();
    offer.seller = offer.seller.to_ascii_uppercase().replacen("0X", "0x", 1);
    assert!(offer.validate().is_err());
}

#[test]
fn record_stem_is_chain_ledger_token() {
    assert_eq!(
        sample_item().record_stem().unwrap(),
        "8453-0x00000000000000000000000000000000000000a2-0x3"
    );
}

fn sample_package() -> RuntimePortableListingPackage {
    RuntimePortableListingPackage {
        schema: "elastos.library.runtime-custody-portable-listing/v1".to_string(),
        mint_id: "0".repeat(64),
        content_id: "content:fixture".to_string(),
        content_cid: "bafyfixturecontent".to_string(),
        metadata_cid: "bafyfixturemetadata".to_string(),
        token_uri: "ipfs://bafyfixturemetadata".to_string(),
        publisher_profile_did: "did:key:z6MkFixture".to_string(),
        display_name: "Demo".to_string(),
        media_identity_base64: Some("ZGVtbw==".to_string()),
        content_identity_base64: None,
        content_access_id: format!("0x{}", "00".repeat(32)),
        key_envelope_identity_base64: "ZGVtbw==".to_string(),
        rights_policy_identity_base64: "ZGVtbw==".to_string(),
        content_key_commitment_base64: "ZGVtbw==".to_string(),
        seller_address: "0x0000000000000000000000000000000000000044".to_string(),
        chain_namespace: "eip155:8453".to_string(),
        network: "base".to_string(),
        ledger: "0x0000000000000000000000000000000000000022".to_string(),
        token_id: "0x3".to_string(),
        operative: "0x0000000000000000000000000000000000000033".to_string(),
        quantity: "0x1".to_string(),
        price: "0x5".to_string(),
        pay_token: "0x0000000000000000000000000000000000000055".to_string(),
        payment_processor: None,
        mint_transaction_hash: Some(format!("0x{}", "00".repeat(32))),
        published_at: Some(1),
    }
}

#[test]
fn market_terms_from_package_extracts_item_and_agreed_offer() {
    let package = sample_package();
    let (item, offer) = market_terms_from_package(&package, sample_item().kid.as_deref().unwrap());

    assert_eq!(item.chain_namespace, package.chain_namespace);
    assert_eq!(item.network, package.network);
    assert_eq!(item.ledger, package.ledger);
    assert_eq!(item.token_id, package.token_id);
    assert_eq!(item.operative, package.operative);
    assert_eq!(item.kid, sample_item().kid);

    assert_eq!(offer.seller, package.seller_address);
    assert_eq!(offer.price, package.price);
    assert_eq!(offer.pay_token, package.pay_token);
    assert_eq!(offer.payment_processor, package.payment_processor);
    assert_eq!(offer.quantity, package.quantity);
}

// --- Task 6: the market purchase record ------------------------------------

fn sample_market_purchase_record() -> (String, RuntimeMarketItem, RuntimeMarketPurchaseRecord) {
    let principal_id = "person:local:market-fixture".to_string();
    let item = sample_item();
    let offer = sample_offer();
    let record = RuntimeMarketPurchaseRecord {
        schema: RUNTIME_MARKET_PURCHASE_SCHEMA_V1.to_string(),
        principal_id: principal_id.clone(),
        account_id: "wallet-account-fixture".to_string(),
        address: "0x0000000000000000000000000000000000000077".to_string(),
        item: item.clone(),
        asset_uri: "elastos://bafyfixturemetadata".to_string(),
        offer,
        attempt_id: "0123456789abcdef0123456789abcdef".to_string(),
        approval_stage: None,
        acquisition_stage: RuntimeCustodyPurchaseStageRecord {
            stage: "buy".to_string(),
            effect_id: "runtime-effect:11111111111111111111111111111111".to_string(),
            approval_request_id: "wallet-request:11111111111111111111111111111111".to_string(),
            request_sha256: format!("sha256:{}", "33".repeat(32)),
            chain_namespace: item.chain_namespace.clone(),
            network: item.network.clone(),
            to: "0x0000000000000000000000000000000000000066".to_string(),
            value: "0x1".to_string(),
            data: "0x".to_string(),
        },
        progress: RuntimeCustodyPurchaseProgress::Pending {
            confirmed_approval: None,
            confirmed_buy: None,
        },
        adopted_mint_id: None,
        created_at: 1,
        updated_at: 1,
    };
    (principal_id, item, record)
}

#[test]
fn market_purchase_round_trips_through_persist_and_load() {
    let dir = tempfile::tempdir().unwrap();
    owner_only_dir(dir.path());
    let (principal_id, item, record) = sample_market_purchase_record();

    persist_runtime_market_purchase(dir.path(), &record).unwrap();
    let loaded = load_runtime_market_purchase(dir.path(), &principal_id, &item)
        .unwrap()
        .expect("a persisted market purchase loads back");
    assert_eq!(loaded, record);
}

#[test]
fn load_runtime_market_purchase_returns_none_when_absent() {
    let dir = tempfile::tempdir().unwrap();
    owner_only_dir(dir.path());
    let (principal_id, item, _record) = sample_market_purchase_record();
    assert!(
        load_runtime_market_purchase(dir.path(), &principal_id, &item)
            .unwrap()
            .is_none()
    );
}

#[test]
fn load_runtime_market_purchase_refuses_a_record_whose_principal_id_differs() {
    let dir = tempfile::tempdir().unwrap();
    owner_only_dir(dir.path());
    let (principal_id, item, mut record) = sample_market_purchase_record();
    // Simulate a swapped/corrupted record file: the bytes at this
    // principal's own path claim a different principal_id.
    record.principal_id = "person:local:someone-else".to_string();
    let path = runtime_market_purchase_path(dir.path(), &principal_id, &item).unwrap();
    write_owner_only_bytes(&path, &serde_json::to_vec(&record).unwrap()).unwrap();

    assert!(load_runtime_market_purchase(dir.path(), &principal_id, &item).is_err());
}

#[cfg(unix)]
#[test]
fn load_runtime_market_purchase_refuses_symlinked_record() {
    let dir = tempfile::tempdir().unwrap();
    owner_only_dir(dir.path());
    let (principal_id, item, record) = sample_market_purchase_record();
    persist_runtime_market_purchase(dir.path(), &record).unwrap();
    let path = runtime_market_purchase_path(dir.path(), &principal_id, &item).unwrap();
    let real = path.with_extension("real");
    fs::rename(&path, &real).unwrap();
    std::os::unix::fs::symlink(&real, &path).unwrap();

    assert!(load_runtime_market_purchase(dir.path(), &principal_id, &item).is_err());
}

// --- Fix round 1: path traversal via unvalidated item fields ---------------

/// Recursively lists every regular file under `root`, relative to `root`.
/// Used to prove a refused operation touched the filesystem nowhere at all,
/// not merely "somewhere the test didn't happen to check".
fn list_files_recursively(root: &Path) -> Vec<std::path::PathBuf> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };
            if file_type.is_dir() {
                walk(&path, root, out);
            } else {
                out.push(path.strip_prefix(root).unwrap_or(&path).to_path_buf());
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

fn traversal_shaped_ledger_item() -> RuntimeMarketItem {
    let mut item = sample_item();
    item.ledger = "../../../../elsewhere".to_string();
    item
}

fn traversal_shaped_token_id_item() -> RuntimeMarketItem {
    let mut item = sample_item();
    item.token_id = "../../../../elsewhere".to_string();
    item
}

#[test]
fn record_stem_refuses_a_traversal_shaped_ledger() {
    assert!(traversal_shaped_ledger_item().record_stem().is_err());
}

#[test]
fn record_stem_refuses_a_traversal_shaped_token_id() {
    assert!(traversal_shaped_token_id_item().record_stem().is_err());
}

#[test]
fn runtime_market_purchase_path_refuses_a_traversal_shaped_ledger() {
    let dir = tempfile::tempdir().unwrap();
    owner_only_dir(dir.path());
    assert!(runtime_market_purchase_path(
        dir.path(),
        "person:local:market-fixture",
        &traversal_shaped_ledger_item()
    )
    .is_err());
}

#[test]
fn runtime_market_purchase_path_refuses_a_traversal_shaped_token_id() {
    let dir = tempfile::tempdir().unwrap();
    owner_only_dir(dir.path());
    assert!(runtime_market_purchase_path(
        dir.path(),
        "person:local:market-fixture",
        &traversal_shaped_token_id_item()
    )
    .is_err());
}

#[test]
fn persist_runtime_market_purchase_refuses_a_traversal_shaped_item_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    owner_only_dir(dir.path());
    let (_principal_id, _item, mut record) = sample_market_purchase_record();
    record.item = traversal_shaped_ledger_item();

    assert!(persist_runtime_market_purchase(dir.path(), &record).is_err());
    assert_eq!(
        list_files_recursively(dir.path()),
        Vec::<std::path::PathBuf>::new(),
        "a refused persist must not create any file, inside or outside the expected root"
    );
}

#[test]
fn load_runtime_market_purchase_refuses_a_traversal_shaped_item_and_reads_nothing() {
    let dir = tempfile::tempdir().unwrap();
    owner_only_dir(dir.path());
    assert!(load_runtime_market_purchase(
        dir.path(),
        "person:local:market-fixture",
        &traversal_shaped_ledger_item()
    )
    .is_err());
    assert_eq!(
        list_files_recursively(dir.path()),
        Vec::<std::path::PathBuf>::new(),
        "a refused load must not create any file"
    );
}

#[test]
fn persist_runtime_market_purchase_serializes_concurrent_writers() {
    let dir = tempfile::tempdir().unwrap();
    owner_only_dir(dir.path());
    let (principal_id, item, base_record) = sample_market_purchase_record();
    let account_ids: Vec<String> = (0..8)
        .map(|index| format!("wallet-account-fixture-{index}"))
        .collect();
    let handles: Vec<_> = account_ids
        .iter()
        .cloned()
        .map(|account_id| {
            let dir = dir.path().to_path_buf();
            let mut record = base_record.clone();
            record.account_id = account_id;
            std::thread::spawn(move || persist_runtime_market_purchase(&dir, &record).unwrap())
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }

    assert!(
        !runtime_market_purchase_lock_test_overlap_detected(&runtime_market_purchase_lock_path(
            dir.path(),
            &principal_id
        )),
        "two persist_runtime_market_purchase calls held the market purchase lock concurrently"
    );
    let persisted = load_runtime_market_purchase(dir.path(), &principal_id, &item)
        .unwrap()
        .expect("a market purchase record survives concurrent writers");
    assert!(
        account_ids.contains(&persisted.account_id),
        "persisted record must be exactly one writer's complete record, not a mix"
    );
}

/// The listing-package purchase (`buy`) of `sample_item()`'s item, as that
/// path records one before its first effect.
fn sample_listing_purchase(
    principal_id: &str,
) -> crate::protected_content_runtime::RuntimeCustodyPurchaseRecord {
    let item = sample_item();
    crate::protected_content_runtime::RuntimeCustodyPurchaseRecord {
        schema: crate::protected_content_runtime::RUNTIME_PURCHASE_SCHEMA_V1.to_string(),
        principal_id: principal_id.to_string(),
        profile_did: "did:key:z6MkFixture".to_string(),
        mint_id: "cd".repeat(32),
        content_id: "content:fixture".to_string(),
        cid: "bafyfixturecontent".to_string(),
        listing_sha256: format!("sha256:{}", "88".repeat(32)),
        seller_address: sample_offer().seller,
        chain_namespace: item.chain_namespace.clone(),
        network: item.network.clone(),
        ledger: item.ledger.clone(),
        token_id: item.token_id.clone(),
        operative: item.operative.clone(),
        price: "0x5".to_string(),
        pay_token: sample_offer().pay_token,
        payment_processor: None,
        availability_receipt_digest: format!("sha256:{}", "99".repeat(32)),
        account_id: "wallet-account-fixture".to_string(),
        address: "0x0000000000000000000000000000000000000077".to_string(),
        approval_stage: None,
        acquisition_stage: RuntimeCustodyPurchaseStageRecord {
            stage: "buy".to_string(),
            effect_id: "runtime-effect:22222222222222222222222222222222".to_string(),
            approval_request_id: "wallet-request:22222222222222222222222222222222".to_string(),
            request_sha256: format!("sha256:{}", "44".repeat(32)),
            chain_namespace: item.chain_namespace.clone(),
            network: item.network.clone(),
            to: "0x0000000000000000000000000000000000000066".to_string(),
            value: "0x1".to_string(),
            data: "0x".to_string(),
        },
        acquisition: crate::protected_content_runtime::RuntimeCustodyAcquisitionV1::Bought,
        capsule_uri: None,
        progress: RuntimeCustodyPurchaseProgress::Pending {
            confirmed_approval: None,
            confirmed_buy: None,
        },
        created_at: 1,
        updated_at: 1,
    }
}

/// R29 (SM-I2): the two purchase paths start at most one purchase of an item
/// between them, however their creates interleave -- each checks the other's
/// record under the same lock before it writes its own.
#[test]
fn market_and_listing_purchase_creates_never_both_start_one_item() {
    for _ in 0..24 {
        let dir = tempfile::tempdir().unwrap();
        owner_only_dir(dir.path());
        let (principal_id, item, market) = sample_market_purchase_record();
        let listing = sample_listing_purchase(&principal_id);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let market_thread = {
            let dir = dir.path().to_path_buf();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                matches!(
                    create_runtime_market_purchase(&dir, &market).unwrap(),
                    RuntimeMarketCreateOutcome::Created
                )
            })
        };
        let listing_thread = {
            let dir = dir.path().to_path_buf();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                create_runtime_listing_purchase_unless_market(&dir, &listing)
                    .unwrap()
                    .is_none()
            })
        };
        let market_created = market_thread.join().unwrap();
        let listing_created = listing_thread.join().unwrap();
        assert!(
            market_created ^ listing_created,
            "exactly one path may start the purchase (market: {market_created}, listing: {listing_created})"
        );
        let market_on_disk = load_runtime_market_purchase(dir.path(), &principal_id, &item)
            .unwrap()
            .is_some();
        let listing_on_disk =
            crate::protected_content_runtime::load_runtime_custody_purchase_for_chain_item(
                dir.path(),
                &principal_id,
                &item.chain_namespace,
                &item.ledger,
                &item.token_id,
            )
            .unwrap()
            .is_some();
        assert_eq!(
            (market_on_disk, listing_on_disk),
            (market_created, listing_created)
        );
    }
}

/// R41: retiring a listing-package attempt removes only the attempt it
/// names -- the same buy effect, created at the same moment. A newer attempt
/// recorded for the same mint is left exactly as it is.
#[test]
fn retire_runtime_listing_purchase_removes_only_the_named_attempt() {
    let dir = tempfile::tempdir().unwrap();
    owner_only_dir(dir.path());
    let principal_id = "principal-retire-fixture";
    let item = sample_item();
    let recorded = sample_listing_purchase(principal_id);
    crate::protected_content_runtime::persist_runtime_custody_purchase(dir.path(), &recorded)
        .unwrap();
    let still_recorded = || {
        crate::protected_content_runtime::load_runtime_custody_purchase_for_chain_item(
            dir.path(),
            principal_id,
            &item.chain_namespace,
            &item.ledger,
            &item.token_id,
        )
        .unwrap()
        .is_some()
    };

    let mut other_effect = recorded.clone();
    other_effect.acquisition_stage.effect_id =
        "runtime-effect:33333333333333333333333333333333".to_string();
    assert!(!retire_runtime_listing_purchase(dir.path(), &other_effect).unwrap());
    let mut older = recorded.clone();
    older.created_at = 0;
    assert!(!retire_runtime_listing_purchase(dir.path(), &older).unwrap());
    assert!(still_recorded(), "another attempt's record is kept");

    assert!(retire_runtime_listing_purchase(dir.path(), &recorded).unwrap());
    assert!(!still_recorded());
    assert!(!retire_runtime_listing_purchase(dir.path(), &recorded).unwrap());
}

/// R41: the retirement runs under the market-purchase lock, the lock R29's
/// cross-path checks hold, so it never interleaves with a create on either
/// path.
#[test]
fn retire_runtime_listing_purchase_holds_the_market_purchase_lock() {
    let dir = tempfile::tempdir().unwrap();
    owner_only_dir(dir.path());
    let principal_id = "principal-retire-lock-fixture";
    let recorded = sample_listing_purchase(principal_id);
    crate::protected_content_runtime::persist_runtime_custody_purchase(dir.path(), &recorded)
        .unwrap();
    let lock_path = runtime_market_purchase_lock_path(dir.path(), principal_id);
    owner_only_dir(lock_path.parent().unwrap());
    let held = elastos_protected_content_runtime::ExclusiveFileLock::acquire(&lock_path).unwrap();
    let (done, finished) = std::sync::mpsc::channel();
    let retirer = {
        let dir = dir.path().to_path_buf();
        std::thread::spawn(move || {
            let retired = retire_runtime_listing_purchase(&dir, &recorded).unwrap();
            done.send(retired).unwrap();
        })
    };
    assert!(
        finished
            .recv_timeout(std::time::Duration::from_millis(300))
            .is_err(),
        "the retirement ran without the market-purchase lock"
    );
    drop(held);
    assert!(finished
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap());
    retirer.join().unwrap();
}

/// R43 (m2): the cross-path scan skips only a record that fails to parse or
/// validate. A record it cannot read at all is an error, so a purchase that
/// may be under way is never taken for an absent one.
#[cfg(unix)]
#[test]
fn listing_purchase_scan_skips_an_invalid_record_and_refuses_an_unreadable_one() {
    let dir = tempfile::tempdir().unwrap();
    owner_only_dir(dir.path());
    let principal_id = "principal-scan-fixture";
    let item = sample_item();
    let recorded = sample_listing_purchase(principal_id);
    crate::protected_content_runtime::persist_runtime_custody_purchase(dir.path(), &recorded)
        .unwrap();
    let record_path = dir.path().join(
        list_files_recursively(dir.path())
            .into_iter()
            .find(|path| path.ends_with(format!("{}.json", recorded.mint_id)))
            .unwrap(),
    );
    let scan = || {
        crate::protected_content_runtime::load_runtime_custody_purchase_for_chain_item(
            dir.path(),
            principal_id,
            &item.chain_namespace,
            &item.ledger,
            &item.token_id,
        )
    };
    // Another mint's record that is not a purchase at all is skipped.
    write_owner_only_bytes(
        &record_path.with_file_name(format!("{}.json", "ef".repeat(32))),
        b"not a purchase record",
    )
    .unwrap();
    assert!(scan().unwrap().is_some());

    fs::set_permissions(&record_path, fs::Permissions::from_mode(0o000)).unwrap();
    let unreadable = scan();
    fs::set_permissions(&record_path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(
        unreadable.is_err(),
        "an unreadable record was skipped: {unreadable:?}"
    );
}

/// R46: a record written before R46 names its item's KID; one `buy_offer`
/// writes now may not know it yet. Both load, and the one without a KID
/// states none rather than inventing one.
#[test]
fn market_purchase_records_load_with_a_recorded_kid_and_without_one() {
    let dir = tempfile::tempdir().unwrap();
    owner_only_dir(dir.path());
    let (principal_id, item, record) = sample_market_purchase_record();
    persist_runtime_market_purchase(dir.path(), &record).unwrap();
    let path = runtime_market_purchase_path(dir.path(), &principal_id, &item).unwrap();

    let mut written = serde_json::to_value(&record).unwrap();
    written["item"]["kid"] = serde_json::json!(format!("0x{}", "11".repeat(16)));
    write_owner_only_bytes(&path, &serde_json::to_vec(&written).unwrap()).unwrap();
    let loaded = load_runtime_market_purchase(dir.path(), &principal_id, &item)
        .unwrap()
        .expect("a record naming its KID loads");
    assert_eq!(
        serde_json::to_value(&loaded.item).unwrap()["kid"],
        serde_json::json!(format!("0x{}", "11".repeat(16)))
    );

    written["item"].as_object_mut().unwrap().remove("kid");
    write_owner_only_bytes(&path, &serde_json::to_vec(&written).unwrap()).unwrap();
    let loaded = load_runtime_market_purchase(dir.path(), &principal_id, &item)
        .unwrap()
        .expect("a record without a KID loads");
    let kid = serde_json::to_value(&loaded.item)
        .unwrap()
        .get("kid")
        .cloned();
    assert!(kid.as_ref().is_none_or(|kid| kid.is_null()), "{kid:?}");
}
