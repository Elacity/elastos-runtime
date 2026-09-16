//! The Elacity listing metadata folder a mint publishes as its token URI.
//!
//! What a marketplace reads is not what this Runtime records. The runtime's own
//! `elastos.protected-content.metadata/v1` document carries internal identities
//! and base64 identity blobs, which is exactly right for the portable-listing
//! round trip and useless to an indexer: it has no title, no description, no
//! cover, and no `media.uri`. A mint that published only that completed on
//! chain and then failed to appear as anything a buyer could recognise.
//!
//! So the token URI points at a DIRECTORY in the shape `base.ela.city` indexes,
//! mirroring the audited PC2 creator that Elacity's own marketplace was built
//! against:
//!
//! | file | what reads it |
//! | --- | --- |
//! | `metadata.json` | the indexer: `name`, `description`, `image`, `media.uri`, `media.contentType`, `properties.publisher`, `kid` |
//! | `content.json` | referenced by `media.object = self://content.json` |
//! | `contract.json` | referenced by `properties.contract`; pricing and supply |
//! | `0000…0001.json` | the Access Token type |
//! | `0000…0002.json` | the Royalty Share type |
//! | `0000…0003.json` | the Distribution Right type, resale listings only |
//!
//! Two interop facts are load-bearing and easy to break silently:
//!
//! * `metadata.json` must be at the directory ROOT, and the on-chain token URI
//!   must name the directory rather than the file. The Operative appends its
//!   own suffixes to the base URI, so a URI that already ends in
//!   `/metadata.json` resolves to `…/metadata.json/contract.json`.
//! * `kid` must equal the on-chain `contentId`: `0x` plus the 32 hex digits of
//!   the bytes16 content access id. The content market rejects metadata whose
//!   `kid` does not match, and the indexer keys its catalog on it.
//!
//! The runtime's own `_elastos_object.json` stays in the directory. The content
//! provider appends it, the runtime's verifier and materialization need it, and
//! nothing outside this Runtime looks at it.
//!
//! The runtime's own document shares this directory as `manifest.json`. It
//! could not keep the name `metadata.json`: that is the name the token URI
//! resolves to, and the Elacity document is the one that has to be there.
//! `verify_runtime_portable_metadata` reads `manifest.json` first and falls
//! back to `metadata.json` for listings published before the rename.

use elastos_protected_content_provider_contracts::ELASTOS_PQ_PROTECTION_SCHEME_V1;
use serde_json::{json, Value};

use crate::library::RuntimeCustodyListingTerms;

/// The canonical Elacity token-metadata schemas. These exact URIs are what the
/// marketplace indexer matches on, so they are values, not descriptions.
const ELACITY_ASSET_SCHEMA: &str =
    "https://raw.githubusercontent.com/Elacity/wiki/main/metadata/schemas/asset/v1.1/schema.json";
const ELACITY_CONTENT_SCHEMA: &str =
    "https://raw.githubusercontent.com/Elacity/wiki/main/metadata/schemas/content/v1.0/schema.json";
const ELACITY_MCO_SCHEMA: &str =
    "https://raw.githubusercontent.com/Elacity/wiki/main/metadata/schemas/mco/v1.0/schema.json";
const ELACITY_ACCESS_TOKEN_SCHEMA: &str = "https://raw.githubusercontent.com/Elacity/wiki/main/metadata/schemas/access-token/v1.0/schema.json";
const ELACITY_ROYALTY_SCHEMA: &str =
    "https://raw.githubusercontent.com/Elacity/wiki/main/metadata/schemas/royalty/v1.0/schema.json";

/// Token-type filenames, named by the 32-byte token id in hex. Token 1 is the
/// Access Token and token 2 the Royalty Share.
pub(crate) const ACCESS_TOKEN_FILE: &str =
    "0000000000000000000000000000000000000000000000000000000000000001.json";
pub(crate) const ROYALTY_SHARE_FILE: &str =
    "0000000000000000000000000000000000000000000000000000000000000002.json";

/// Everything the folder needs that is not the creator's own listing copy.
pub(crate) struct ElacityMetadataInputs<'a> {
    /// CID of the published ciphertext, which becomes `media.uri`.
    pub encrypted_content_cid: &'a str,
    /// The declared content type: a MIME, never a category word.
    pub content_type: &'a str,
    /// Size of the CLEAR content in bytes.
    pub plaintext_bytes: u64,
    /// `0x` + 32 hex digits: the bytes16 content access id, which is the
    /// on-chain `contentId`.
    pub kid_0x: &'a str,
    /// EVM address credited as the publisher.
    pub publisher_address: &'a str,
    /// The chain the mint settles on.
    pub chain_id: u64,
    /// Ledger (Operative) address the listing belongs to.
    pub ledger: &'a str,
    /// Copies offered, as a decimal count.
    pub copies: u64,
    /// Price in base units, decimal.
    pub price: &'a str,
    /// Cover image reference (`ipfs://…`), empty when the creator sent none.
    pub image: &'a str,
    /// The custody committee the release protocol must reach, as public
    /// identity facts only.
    pub protection: Value,
    /// Fallback display name when the creator supplied no listing at all.
    pub fallback_name: &'a str,
}

/// The public custody descriptor carried in `asset.protections[0]`.
///
/// Identities and thresholds only. The sealed shares, the envelope and the
/// commitment stay in the runtime's own document and in the custody nodes: a
/// buyer needs to know which committee to ask and under what policy, never any
/// material that could shortcut asking.
///
/// `protectionType` is [`ELASTOS_PQ_PROTECTION_SCHEME_V1`] itself rather than a
/// copy of its text. It is the same fact the `pssh` box in the init segment
/// states as its `protection_scheme`, and the two must agree or a reader that
/// found the folder first would speak a release protocol the box does not
/// implement. Naming it twice is how they drifted once already, so there is now
/// one definition and a test that holds the folder to it.
///
/// There is deliberately no second name for the scheme beside it. The threshold
/// and node count are carried here as numbers, so a string that also spelled
/// them out could contradict the data next to it; and the suites — how the
/// samples are encrypted, and which key encapsulation the released key arrives
/// under — are already stated canonically in the `pssh` payload
/// (`content_encryption`, `key_encapsulation`). Restating either here would be
/// a second source for a fact that already has one.
pub(crate) fn runtime_custody_protection(
    threshold: u32,
    node_count: u32,
    rights_policy_identity_base64: &str,
) -> Value {
    json!({
        "protectionType": ELASTOS_PQ_PROTECTION_SCHEME_V1,
        // How many of how many must agree before a release happens.
        "threshold": threshold,
        "node_count": node_count,
        // Which rights policy governs that release. The custody pool and epoch
        // identities would say more, but they live on the mint intent rather
        // than the draft this is built from; naming the policy states what
        // governs release without inventing a fact this site cannot see.
        "rights_policy_identity_base64": rights_policy_identity_base64,
    })
}

/// Build every file in the metadata directory, as (filename, bytes) pairs.
///
/// `metadata.json` is first so a reader that takes the directory's first entry
/// still finds the document the token URI is meant to resolve to.
pub(crate) fn elacity_metadata_files(
    listing: Option<&RuntimeCustodyListingTerms>,
    inputs: &ElacityMetadataInputs<'_>,
) -> Result<Vec<(String, Vec<u8>)>, serde_json::Error> {
    let title = listing
        .map(|listing| listing.title.as_str())
        .filter(|title| !title.is_empty())
        .unwrap_or(inputs.fallback_name);
    let description = listing.map_or("", |listing| listing.description.as_str());
    let category = listing.map_or("", |listing| listing.category.as_str());
    let tags: Vec<String> = listing
        .map(|listing| listing.tags.clone())
        .unwrap_or_default();

    let files = vec![
        (
            "metadata.json".to_string(),
            metadata_json(inputs, title, description, category, &tags, listing),
        ),
        ("content.json".to_string(), content_json(inputs, title)),
        ("contract.json".to_string(), contract_json(inputs, title)),
        (
            ACCESS_TOKEN_FILE.to_string(),
            token_type_json(
                ELACITY_ACCESS_TOKEN_SCHEMA,
                "AccessToken",
                "Access Token",
                "Allow owner to access the content",
                inputs,
                title,
                None,
            ),
        ),
        (
            ROYALTY_SHARE_FILE.to_string(),
            token_type_json(
                ELACITY_ROYALTY_SCHEMA,
                "RoyaltyShare",
                "Royalty Share",
                "10 shares = 1% of revenue",
                inputs,
                title,
                Some(1),
            ),
        ),
    ];

    files
        .into_iter()
        .map(|(name, value)| serde_json::to_vec(&value).map(|bytes| (name, bytes)))
        .collect()
}

fn metadata_json(
    inputs: &ElacityMetadataInputs<'_>,
    title: &str,
    description: &str,
    category: &str,
    tags: &[String],
    listing: Option<&RuntimeCustodyListingTerms>,
) -> Value {
    json!({
        "schema": ELACITY_ASSET_SCHEMA,
        "version": "1.1",
        "name": title,
        "description": description,
        // Empty means the indexer falls back to its own type icon rather than
        // showing a broken image.
        "image": inputs.image,
        "category": category,
        "media": {
            "uri": format!("ipfs://{}", inputs.encrypted_content_cid),
            "contentType": inputs.content_type,
            "mimeType": inputs.content_type,
            "object": "self://content.json",
            "protectionType": [ELASTOS_PQ_PROTECTION_SCHEME_V1],
            "size": inputs.plaintext_bytes,
        },
        "asset": {
            "cid": inputs.encrypted_content_cid,
            "mimeType": inputs.content_type,
            "size": inputs.plaintext_bytes,
            "encrypted": true,
            "protections": [inputs.protection.clone()],
            "kid": inputs.kid_0x,
        },
        "properties": {
            "chainId": inputs.chain_id,
            "ledger": inputs.ledger,
            "publisher": inputs.publisher_address,
            "contract": "self://contract.json",
            "labelType": "Creator",
            "tags": tags,
            "categories": if category.is_empty() { Vec::new() } else { vec![category.to_string()] },
            "adult": listing.is_some_and(|listing| listing.adult),
            "licensing": listing.and_then(|listing| listing.licensing.clone()),
            "legal": listing.and_then(|listing| listing.legal_attestation.clone()),
            "kid": inputs.kid_0x,
        },
        "attributes": [
            { "trait_type": "Content-Type", "value": inputs.content_type },
            { "trait_type": "Size", "value": inputs.plaintext_bytes },
            { "trait_type": "Encrypted", "value": true },
            { "trait_type": "Supply", "value": inputs.copies },
        ],
        // The indexer keys on `metadata.kid || metadata.properties.kid`, so it
        // is stated at both levels deliberately rather than by duplication.
        "kid": inputs.kid_0x,
    })
}

fn content_json(inputs: &ElacityMetadataInputs<'_>, title: &str) -> Value {
    json!({
        "schema": ELACITY_CONTENT_SCHEMA,
        "version": "1.0",
        "title": title,
        "type": inputs.content_type,
        "description": "Details about the content, technical informations, etc.",
        "image": inputs.image,
        "properties": {
            "size": inputs.plaintext_bytes,
            "protectionType": [ELASTOS_PQ_PROTECTION_SCHEME_V1],
            "kid": inputs.kid_0x,
        },
        "attributes": [
            { "trait_type": "Content-Type", "value": inputs.content_type },
            { "trait_type": "Size", "value": inputs.plaintext_bytes },
            { "trait_type": "Encrypted", "value": true },
        ],
    })
}

fn contract_json(inputs: &ElacityMetadataInputs<'_>, title: &str) -> Value {
    json!({
        "schema": ELACITY_MCO_SCHEMA,
        "version": "1.0",
        "title": format!("Contract - {title}"),
        "type": "MCO",
        "description": "Media Contract Ontology (MCO) formatted in JSON",
        // Carried although the wiki's mco schema does not list it: a contract
        // document with no image renders as a blank tile wherever one is shown.
        "image": inputs.image,
        "properties": {
            "chainId": inputs.chain_id,
            "channel": inputs.ledger,
            "initialPrice": {
                "value": inputs.price,
                // Native currency. A non-native pay token would name its own
                // contract and decimals here; this Runtime mints against the
                // native token only, so stating anything else would be a claim
                // the chain does not back.
                "paymentToken": "0x0000000000000000000000000000000000000000",
                "paymentDecimals": 18,
            },
        },
        "attributes": [
            { "trait_type": "Content-Type", "value": inputs.content_type },
            { "trait_type": "OpType", "value": "buy_once" },
            { "trait_type": "Supply", "value": inputs.copies },
            // Resale is not offered by this Runtime yet, so it is stated as
            // false rather than omitted: an absent flag reads as unknown.
            { "trait_type": "Resell-Allowed", "value": false },
            { "trait_type": "RRL-Percent", "value": 0 },
        ],
    })
}

fn token_type_json(
    schema: &str,
    kind: &str,
    name: &str,
    description: &str,
    inputs: &ElacityMetadataInputs<'_>,
    title: &str,
    decimals: Option<u32>,
) -> Value {
    let mut value = json!({
        "schema": schema,
        "version": "1.0",
        "type": kind,
        "name": name,
        "description": description,
        "image": inputs.image,
        "properties": { "kid": inputs.kid_0x, "title": title },
    });
    if let Some(decimals) = decimals {
        value["decimals"] = json!(decimals);
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs() -> ElacityMetadataInputs<'static> {
        ElacityMetadataInputs {
            encrypted_content_cid: "bafycidofciphertext",
            content_type: "image/png",
            plaintext_bytes: 4096,
            kid_0x: "0x51515151515151515151515151515151",
            publisher_address: "0xab5028bdbb0826ad6f1885478e421db677b0001a",
            chain_id: 8453,
            ledger: "0x0ebac909d31ef0074495e752c0cf4ea49ba13c41",
            copies: 1000,
            price: "100000",
            image: "ipfs://bafycidofcover",
            protection: runtime_custody_protection(2, 3, "cG9saWN5"),
            fallback_name: "protected-runtime-proof.png",
        }
    }

    fn parse(files: &[(String, Vec<u8>)], name: &str) -> Value {
        let (_, bytes) = files
            .iter()
            .find(|(file, _)| file == name)
            .unwrap_or_else(|| panic!("the directory must contain {name}"));
        serde_json::from_slice(bytes).expect("each emitted file must be valid JSON")
    }

    /// The directory shape a marketplace indexes. Five files, `metadata.json`
    /// first so the token URI's target is the directory's first entry.
    #[test]
    fn the_directory_carries_every_elacity_file_with_metadata_first() {
        let files = elacity_metadata_files(None, &inputs()).unwrap();
        let names: Vec<&str> = files.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "metadata.json",
                "content.json",
                "contract.json",
                ACCESS_TOKEN_FILE,
                ROYALTY_SHARE_FILE,
            ]
        );
    }

    /// The two interop facts that break a listing silently when wrong: the kid
    /// must equal the on-chain contentId at both levels the indexer reads, and
    /// `media.contentType` must be a MIME rather than a category word.
    #[test]
    fn metadata_states_the_contract_kid_and_a_real_mime() {
        let files = elacity_metadata_files(None, &inputs()).unwrap();
        let metadata = parse(&files, "metadata.json");
        assert_eq!(metadata["kid"], "0x51515151515151515151515151515151");
        assert_eq!(metadata["properties"]["kid"], metadata["kid"]);
        assert_eq!(metadata["asset"]["kid"], metadata["kid"]);
        assert_eq!(metadata["media"]["contentType"], "image/png");
        assert_eq!(metadata["media"]["uri"], "ipfs://bafycidofciphertext");
        assert_eq!(
            metadata["properties"]["publisher"],
            "0xab5028bdbb0826ad6f1885478e421db677b0001a"
        );
    }

    /// Without listing terms the folder is still valid and still indexable: the
    /// name falls back to the object's own, and nothing claims a title,
    /// description or cover the creator never gave.
    #[test]
    fn a_publish_without_listing_terms_still_produces_an_indexable_folder() {
        let files = elacity_metadata_files(None, &inputs()).unwrap();
        let metadata = parse(&files, "metadata.json");
        assert_eq!(metadata["name"], "protected-runtime-proof.png");
        assert_eq!(metadata["description"], "");
        assert_eq!(metadata["category"], "");
        assert_eq!(metadata["properties"]["tags"], json!([]));
        assert_eq!(metadata["properties"]["adult"], false);
        assert_eq!(metadata["properties"]["licensing"], Value::Null);
    }

    /// The custody descriptor is public identity only. Anything that could
    /// shortcut asking the committee must not be in a document the whole
    /// network can read.
    #[test]
    fn the_protection_descriptor_carries_identity_and_no_material() {
        let files = elacity_metadata_files(None, &inputs()).unwrap();
        let protection = &parse(&files, "metadata.json")["asset"]["protections"][0];
        // The same fact the `pssh` box states, from the same definition. A
        // reader that finds the folder first and the box second must not be
        // told two different release protocols -- which is exactly what
        // happened while these were two constants.
        assert_eq!(
            protection["protectionType"],
            elastos_protected_content_provider_contracts::ELASTOS_PQ_PROTECTION_SCHEME_V1
        );
        assert_eq!(protection["threshold"], 2);
        assert_eq!(protection["node_count"], 3);
        assert_eq!(protection["rights_policy_identity_base64"], "cG9saWN5");
        // Four fields, not five: there is no second name for the scheme. The
        // threshold is data here, so a string spelling "2of3" could contradict
        // the numbers beside it, and the suites already have one home in the
        // `pssh` payload.
        let fields: Vec<&String> = protection.as_object().unwrap().keys().collect();
        assert_eq!(fields.len(), 4, "{protection}");
        assert!(
            !protection.as_object().unwrap().contains_key("scheme"),
            "{protection}"
        );
        for forbidden in ["shares", "key_envelope", "commitment", "cek", "secret"] {
            assert!(
                protection.get(forbidden).is_none(),
                "{forbidden} must not travel"
            );
        }
    }

    #[test]
    fn listing_terms_reach_every_document_that_shows_them() {
        let listing = RuntimeCustodyListingTerms {
            title: "My asset".to_string(),
            description: "What it is".to_string(),
            category: "art".to_string(),
            tags: vec!["one".to_string(), "two".to_string()],
            thumbnail: None,
            royalties: Vec::new(),
            adult: true,
            licensing: Some(json!({ "ai_training": true })),
            legal_attestation: Some(json!({ "owns_rights": true })),
        };
        let files = elacity_metadata_files(Some(&listing), &inputs()).unwrap();
        let metadata = parse(&files, "metadata.json");
        assert_eq!(metadata["name"], "My asset");
        assert_eq!(metadata["description"], "What it is");
        assert_eq!(metadata["category"], "art");
        assert_eq!(metadata["properties"]["tags"], json!(["one", "two"]));
        assert_eq!(metadata["properties"]["adult"], true);
        assert_eq!(
            metadata["properties"]["licensing"],
            json!({ "ai_training": true })
        );
        assert_eq!(
            metadata["properties"]["legal"],
            json!({ "owns_rights": true })
        );
        // The title travels to every document that displays one, so a listing
        // cannot show two different names for the same asset.
        assert_eq!(parse(&files, "content.json")["title"], "My asset");
        assert_eq!(
            parse(&files, "contract.json")["title"],
            "Contract - My asset"
        );
        assert_eq!(
            parse(&files, ACCESS_TOKEN_FILE)["properties"]["title"],
            "My asset"
        );
        assert_eq!(
            parse(&files, ROYALTY_SHARE_FILE)["properties"]["title"],
            "My asset"
        );
    }

    #[test]
    fn contract_states_supply_and_price_in_base_units() {
        let files = elacity_metadata_files(None, &inputs()).unwrap();
        let contract = parse(&files, "contract.json");
        assert_eq!(contract["properties"]["initialPrice"]["value"], "100000");
        assert_eq!(
            contract["properties"]["initialPrice"]["paymentDecimals"],
            18
        );
        let supply = contract["attributes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|attribute| attribute["trait_type"] == "Supply")
            .unwrap()["value"]
            .clone();
        assert_eq!(supply, json!(1000));
    }

    /// The royalty share document declares one decimal, so 10 shares read as
    /// 1% rather than 10%.
    #[test]
    fn the_royalty_share_declares_its_decimals_and_the_access_token_does_not() {
        let files = elacity_metadata_files(None, &inputs()).unwrap();
        assert_eq!(parse(&files, ROYALTY_SHARE_FILE)["decimals"], 1);
        assert!(parse(&files, ACCESS_TOKEN_FILE).get("decimals").is_none());
    }
}
