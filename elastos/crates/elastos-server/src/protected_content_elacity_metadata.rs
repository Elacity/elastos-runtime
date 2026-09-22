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
    /// The authority gateway that governs this asset, stated at mint time so a
    /// reader does not have to resolve it from whatever a runtime happens to
    /// have registered afterwards. Empty when the network has no market
    /// configured, in which case it is recovered from the mint's own
    /// `ItemListed` -- the gateway that emitted it IS the authority.
    pub authority: &'a str,
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
    /// RFC 3339 timestamp for `createdAt`. Passed in rather than read from a
    /// clock here so the same inputs always produce the same folder, which is
    /// what lets a test compare one byte for byte.
    pub created_at: &'a str,
    /// How the creator chose to let people reach the asset. It decides the
    /// `opType` the mint encodes, so the document states it rather than
    /// assuming one: a folder that claims resale for an asset minted without
    /// it describes a market that does not exist.
    pub access_method: elastos_protected_content_runtime::RuntimeMintAccessMethod,
    /// The resale cut in deci-percent, present only for buy and resell.
    pub reseller_cut: Option<u16>,
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
    identities: ProtectedContentIdentities<'_>,
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
        "rights_policy_identity_base64": identities.rights_policy,
        // What a buyer needs in order to OPEN what they bought.
        //
        // These three are already public: the listing package carries them and
        // sits on IPFS, fetched by CID with no authentication. What they were
        // not, until now, is REACHABLE -- findable only from a link a creator
        // hands out, rather than from the document the token URI names, which
        // anyone can resolve from chain.
        //
        // That difference is the whole of it. Someone who finds this asset in
        // an index can buy it from what the chain says, and could then hold
        // something they cannot open. With these, they can ask custody for the
        // key (the envelope), check that the key belongs to this content (the
        // commitment), and verify the bytes they fetched are the bytes that
        // were sold (the content identity).
        //
        // None of them is key material, and none of them grants anything: the
        // chain still decides whether this account may open it.
        "key_envelope_identity_base64": identities.key_envelope,
        "content_key_commitment_base64": identities.content_key_commitment,
        "content_identity_base64": identities.content_identity,
    })
}

/// The identities a buyer needs to open what they bought, gathered so that
/// adding one is a field here rather than another positional argument.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ProtectedContentIdentities<'a> {
    pub rights_policy: &'a str,
    pub key_envelope: &'a str,
    pub content_key_commitment: &'a str,
    /// The chunked payload identity for an object, or the media identity for
    /// a media listing: one of the two, in the same encoding the listing
    /// package carries.
    pub content_identity: &'a str,
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

/// Formats unix seconds as an RFC 3339 UTC timestamp.
///
/// Written here rather than taken from a date crate: the workspace has none,
/// and `createdAt` is the only place this Runtime needs one. The civil-date
/// conversion is the standard days-to-y/m/d algorithm, shifted so the era
/// starts in March and leap days fall at the end of a year.
pub(crate) fn rfc3339_utc(unix_seconds: u64) -> String {
    let days = (unix_seconds / 86_400) as i64;
    let seconds_of_day = unix_seconds % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u64;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    } as u64;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        seconds_of_day / 3600,
        (seconds_of_day % 3600) / 60,
        seconds_of_day % 60,
    )
}

/// The category word a marketplace indexes on, derived from the MIME.
///
/// `media.contentType` in the Elacity asset schema is a category -- "Video",
/// "Audio", "Image", "Text" and "Other" -- not a MIME, and that enum is
/// exactly five words, so a 3D model and a PDF are both Other however much a
/// richer word would suit them. This folder used to
/// publish the MIME there, so an indexer reading it got `video/mp4` where it
/// expected `Video` and could not classify the asset at all. The MIME is still
/// stated, beside it, as `mimeType`.
fn elacity_content_category(content_type: &str) -> &'static str {
    let mime = content_type.split(';').next().unwrap_or("").trim();
    let (top, _) = mime.split_once('/').unwrap_or((mime, ""));
    match top {
        "video" => "Video",
        "audio" => "Audio",
        "image" => "Image",
        "text" => "Text",
        _ => "Other",
    }
}

/// The sample cipher this asset's bytes are actually protected with.
///
/// Media and objects differ, and the difference is load-bearing rather than
/// cosmetic: CENC sample encryption is AES-128-CTR and carries no per-sample
/// authentication tag, so tamper-evidence on that path comes from the staged
/// segment's SHA-256 and length check. An object's chunks are AES-256-GCM and
/// the integrity is in the cipher. Stating each truthfully is the point; the
/// PC2 generator names different strings here and this follows the bytes
/// rather than that text.
fn elacity_content_algorithm(content_type: &str) -> &'static str {
    match elacity_content_category(content_type) {
        "Video" | "Audio" => "AES-128-CTR",
        _ => "AES-256-GCM",
    }
}

fn metadata_json(
    inputs: &ElacityMetadataInputs<'_>,
    title: &str,
    description: &str,
    category: &str,
    tags: &[String],
    listing: Option<&RuntimeCustodyListingTerms>,
) -> Value {
    let mut document = json!({
        "schema": ELACITY_ASSET_SCHEMA,
        "version": "1.1",
        "name": title,
        "description": description,
        // Empty means the indexer falls back to its own type icon rather than
        // showing a broken image.
        "image": inputs.image,
        "category": category,
        // The schema's own top-level field, whose enum is the five words the
        // category helper returns. `media.contentType` is a free string and
        // carries the same answer, which is where the PC2 generator puts it.
        "contentType": elacity_content_category(inputs.content_type),
        "media": {
            "uri": format!("ipfs://{}", inputs.encrypted_content_cid),
            // The category word the schema calls for, with the MIME beside it.
            "contentType": elacity_content_category(inputs.content_type),
            "mimeType": inputs.content_type,
            "object": "self://content.json",
            // An array with at least one entry, which is what the schema
            // declares. The PC2 generator emits a bare string here; the schema
            // is the authority and it says `array of strings, minItems 1`.
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
            "authority": inputs.authority,
            "publisher": inputs.publisher_address,
            "contract": "self://contract.json",
            "labelType": "Independent Creator",
            // How the asset may be acquired, in the schema's own vocabulary
            // and in the terms this mint actually carries.
            "distribution": elacity_distribution(inputs.access_method),
            "tags": tags,
            "categories": if category.is_empty() { Vec::new() } else { vec![category.to_string()] },
            "adult": listing.is_some_and(|listing| listing.adult),
            "kid": inputs.kid_0x,
        },
        "attributes": [
            { "trait_type": "Content-Type", "value": elacity_content_category(inputs.content_type) },
            { "trait_type": "Size", "value": inputs.plaintext_bytes },
            { "trait_type": "Encrypted", "value": true },
            // The operation type the mint encodes, matching the distribution
            // stated above. Stated as a string because that is the shape the
            // indexer reads these traits in.
            { "trait_type": "OpType", "value": inputs.access_method.op_type_code().to_string() },
            { "trait_type": "Resell-Allowed", "value": elacity_resell_allowed(inputs.access_method).to_string() },
            { "trait_type": "Algorithm", "value": elacity_content_algorithm(inputs.content_type) },
            { "trait_type": "Supply", "value": inputs.copies },
        ],
        // The indexer keys on `metadata.kid || metadata.properties.kid`, so it
        // is stated at both levels deliberately rather than by duplication.
        "kid": inputs.kid_0x,
        // When the folder was produced. The schema carries it and a consumer
        // ordering by recency has nothing else in the document to order by.
        "createdAt": inputs.created_at,
    });
    // Absent facts are left out rather than stated as `null`. A consumer
    // syncing this into a database reads a missing key as "not supplied" and a
    // null as "supplied as nothing", and only the first is true here. The
    // category is carried in `properties.categories`, so an empty top-level
    // copy of it said nothing twice.
    if let Some(listing) = listing {
        if let Some(licensing) = listing.licensing.clone() {
            document["properties"]["licensing"] = licensing;
        }
        if let Some(legal) = listing.legal_attestation.clone() {
            document["properties"]["legal"] = legal;
        }
    }
    document
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
            { "trait_type": "OpType", "value": elacity_op_type_label(inputs.access_method) },
            { "trait_type": "Supply", "value": inputs.copies },
            // Stated either way rather than omitted when false: an absent flag
            // reads as unknown.
            { "trait_type": "Resell-Allowed", "value": elacity_resell_allowed(inputs.access_method) },
            // The creator's resale royalty as a percentage, from the
            // deci-percent the chain call carries. Zero when resale is not
            // offered at all.
            { "trait_type": "RRL-Percent", "value": f64::from(inputs.reseller_cut.unwrap_or(0)) / 10.0 },
        ],
    })
}

/// The schema's acquisition phrase for an access method.
fn elacity_distribution(
    method: elastos_protected_content_runtime::RuntimeMintAccessMethod,
) -> &'static str {
    use elastos_protected_content_runtime::RuntimeMintAccessMethod as Method;
    match method {
        // No operative and no listing, so nothing is sold. Whoever the channel
        // admits may play it.
        Method::Free => "Free, play always",
        Method::BuyOnce => "Buy once, play always",
        Method::BuyAndResell => "Buy once, play always, resell",
    }
}

/// The word form of the op type, for the document that states it that way.
fn elacity_op_type_label(
    method: elastos_protected_content_runtime::RuntimeMintAccessMethod,
) -> &'static str {
    use elastos_protected_content_runtime::RuntimeMintAccessMethod as Method;
    match method {
        Method::Free => "free",
        Method::BuyOnce => "buy_once",
        Method::BuyAndResell => "buy_and_resell",
    }
}

/// Whether an owner may resell their access. Only one method creates an
/// operative that permits it.
const fn elacity_resell_allowed(
    method: elastos_protected_content_runtime::RuntimeMintAccessMethod,
) -> bool {
    matches!(
        method,
        elastos_protected_content_runtime::RuntimeMintAccessMethod::BuyAndResell
    )
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
            access_method: elastos_protected_content_runtime::RuntimeMintAccessMethod::BuyAndResell,
            reseller_cut: Some(900),
            encrypted_content_cid: "bafycidofciphertext",
            content_type: "image/png",
            plaintext_bytes: 4096,
            kid_0x: "0x51515151515151515151515151515151",
            publisher_address: "0xab5028bdbb0826ad6f1885478e421db677b0001a",
            chain_id: 8453,
            ledger: "0x0ebac909d31ef0074495e752c0cf4ea49ba13c41",
            authority: "0x00000000000000000000000000000000000000aa",
            copies: 1000,
            price: "100000",
            image: "ipfs://bafycidofcover",
            protection: runtime_custody_protection(
                2,
                3,
                ProtectedContentIdentities {
                    rights_policy: "cG9saWN5",
                    key_envelope: "ZW52ZWxvcGU=",
                    content_key_commitment: "Y29tbWl0bWVudA==",
                    content_identity: "aWRlbnRpdHk=",
                },
            ),
            fallback_name: "protected-runtime-proof.png",
            created_at: "2026-09-22T13:45:00Z",
        }
    }

    /// `media.contentType` is the category word a marketplace indexes on, not
    /// the MIME.
    ///
    /// The schema's vocabulary is Video, Audio, Image, 3D Model, Document. This
    /// folder published the MIME there, so an indexer reading it got
    /// `video/mp4` where it expected `Video` and could not classify the asset.
    /// The MIME is still stated, as `mimeType`, because it is the fact a viewer
    /// needs and the category is the fact a shelf needs.
    #[test]
    fn media_states_the_category_a_marketplace_reads_and_the_mime_beside_it() {
        // The schema's enum is exactly these five words.
        for (mime, category, algorithm) in [
            ("video/mp4", "Video", "AES-128-CTR"),
            ("audio/mp4", "Audio", "AES-128-CTR"),
            ("image/png", "Image", "AES-256-GCM"),
            ("text/plain; charset=utf-8", "Text", "AES-256-GCM"),
            ("model/gltf-binary", "Other", "AES-256-GCM"),
            ("application/pdf", "Other", "AES-256-GCM"),
        ] {
            assert_eq!(elacity_content_category(mime), category, "{mime}");
            assert_eq!(elacity_content_algorithm(mime), algorithm, "{mime}");
        }

        let mut inputs = inputs();
        inputs.content_type = "video/mp4";
        let files = elacity_metadata_files(None, &inputs).unwrap();
        let metadata = parse(&files, "metadata.json");
        assert_eq!(metadata["media"]["contentType"], "Video");
        assert_eq!(metadata["media"]["mimeType"], "video/mp4");
        // The schema declares an array with at least one entry.
        let protection_types = metadata["media"]["protectionType"].as_array().unwrap();
        assert_eq!(protection_types.len(), 1, "{protection_types:?}");
        // And the top-level field the schema names, with its enum value.
        assert_eq!(metadata["contentType"], "Video");
        assert_eq!(metadata["createdAt"], "2026-09-22T13:45:00Z");
        assert_eq!(
            metadata["properties"]["distribution"],
            "Buy once, play always, resell"
        );
        let traits: Vec<&str> = metadata["attributes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|attribute| attribute["trait_type"].as_str().unwrap_or_default())
            .collect();
        for expected in [
            "Content-Type",
            "Encrypted",
            "OpType",
            "Resell-Allowed",
            "Algorithm",
            "Supply",
        ] {
            assert!(
                traits.contains(&expected),
                "{expected} missing from {traits:?}"
            );
        }
    }

    /// The timestamp is produced here, so it is pinned here.
    #[test]
    fn rfc3339_utc_formats_known_instants() {
        assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_utc(1_000_000_000), "2001-09-09T01:46:40Z");
        // A leap day, which the civil-date shift exists to get right.
        assert_eq!(rfc3339_utc(1_709_208_000), "2024-02-29T12:00:00Z");
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

    /// The interop facts that break a listing silently when wrong: the kid must
    /// equal the on-chain contentId at both levels the indexer reads, and the
    /// media block must state both the category a shelf indexes on and the MIME
    /// a viewer needs.
    ///
    /// This assertion used to require a MIME in `media.contentType`, which is
    /// the opposite of the asset schema and of the generator every other
    /// Elacity listing is produced by. The MIME did not stop being stated; it
    /// moved to `mimeType`, where the rest of the system now reads it.
    #[test]
    fn metadata_states_the_contract_kid_and_a_real_mime() {
        let files = elacity_metadata_files(None, &inputs()).unwrap();
        let metadata = parse(&files, "metadata.json");
        assert_eq!(metadata["kid"], "0x51515151515151515151515151515151");
        assert_eq!(metadata["properties"]["kid"], metadata["kid"]);
        assert_eq!(metadata["asset"]["kid"], metadata["kid"]);
        assert_eq!(metadata["media"]["contentType"], "Image");
        assert_eq!(metadata["contentType"], "Image");
        assert_eq!(metadata["media"]["mimeType"], "image/png");
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
        // The three that make a purchase openable by someone who found this
        // asset in an index rather than through the creator's link: which
        // envelope holds the key, that the key belongs to this content, and
        // what the content is.
        //
        // They were deliberately absent, and are deliberately here now. What
        // changed is not the judgement about key material -- none of these is
        // any -- but the reachability of facts that were already public: the
        // listing package carries all three and sits on IPFS, fetched by CID
        // with no authentication. Withholding them from the document the
        // token URI names meant a buyer could pay for something on chain and
        // then hold what they could not open.
        //
        // Each is an IDENTITY: schema names and digests naming the custody
        // pool, epoch and committee authorization. The key shares themselves
        // are held by the custody nodes and released against a rights check,
        // which is unchanged by any of this.
        assert_eq!(protection["key_envelope_identity_base64"], "ZW52ZWxvcGU=");
        assert_eq!(
            protection["content_key_commitment_base64"],
            "Y29tbWl0bWVudA=="
        );
        assert_eq!(protection["content_identity_base64"], "aWRlbnRpdHk=");
        // Seven fields, and no second name for the scheme. The threshold is
        // data here, so a string spelling "2of3" could contradict the numbers
        // beside it, and the suites already have one home in the `pssh`
        // payload.
        let fields: Vec<&String> = protection.as_object().unwrap().keys().collect();
        assert_eq!(fields.len(), 7, "{protection}");
        assert!(
            !protection.as_object().unwrap().contains_key("scheme"),
            "{protection}"
        );
        // Still no material, which is the guarantee that has not moved: an
        // identity names a thing, and none of these carries the thing itself.
        for forbidden in [
            "shares",
            "key_envelope",
            "commitment",
            "cek",
            "secret",
            "key",
        ] {
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
            access_method: elastos_protected_content_runtime::RuntimeMintAccessMethod::BuyOnce,
            reseller_cut: None,
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
