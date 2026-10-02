//! The creator-mint wire, defined once for both sides of it.
//!
//! The Runtime asks a chain provider three questions to mint protected
//! content: what this deployment mints on, what call to make, and what the
//! settled transaction proved. Each answer used to be written twice -- built
//! as a JSON literal in the capsule and declared as a `Deserialize` struct in
//! the gateway -- with nothing but care keeping the two in step.
//!
//! Care was not enough. The gateway's structs are `deny_unknown_fields`, so a
//! field added to one side and not the other is a hard refusal at runtime, and
//! every test in between passes because the tests answer with a hand-written
//! mock rather than with the capsule. Four separate field mismatches reached a
//! person that way -- `op_type_code`, the channel/pay-token reshape,
//! `pay_token_decimals`, and a free mint's null listing -- each found by
//! building and watching it fail.
//!
//! So the shapes live here, in the crate both sides already depend on. The
//! capsule serialises exactly what the gateway deserialises, and a field that
//! exists on one side only stops compiling instead of stopping a mint.

use serde::{Deserialize, Serialize};

/// What a creator may price a sale in, as the deployment offers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtectedContentPayTokenV1 {
    /// What a creator sees. Display only; the address is what is encoded.
    pub symbol: String,
    pub address: String,
    /// What a price in this token MEANS. Stated rather than read from the
    /// token, because a price must not depend on a chain call that can fail or
    /// answer late.
    pub decimals: u8,
}

pub const PROTECTED_CONTENT_CREATOR_MINT_SOURCE_SCHEMA_V1: &str =
    "elastos.chain.protected-content-creator-mint-source/v1";

/// The deployment a mint settles on.
///
/// It names no channel: a mint settles on the channel its creator chose, and a
/// source that named one was read as *the* channel for as long as a Home had
/// only one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtectedContentCreatorMintSourceV1 {
    pub schema: String,
    pub network: String,
    pub chain_namespace: String,
    /// In the order they are offered. THE FIRST IS THE DEFAULT, so the order
    /// is a product decision rather than a formatting one.
    #[serde(default)]
    pub pay_tokens: Vec<ProtectedContentPayTokenV1>,
    pub abi: String,
    pub function: String,
    /// The authority gateway this network's market is configured with. Absent
    /// when the network has no market configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority_gateway_contract: Option<String>,
}

pub const PROTECTED_CONTENT_CREATOR_MINT_SCHEMA_V1: &str =
    "elastos.chain.protected-content-creator-mint/v1";

/// The exact call a creator's mint is, before anyone signs it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtectedContentCreatorMintV1 {
    pub schema: String,
    pub network: String,
    pub chain_namespace: String,
    pub function: String,
    /// The channel this mint settles on: the creator's choice, echoed so the
    /// caller can check the capsule encoded the one it asked for.
    pub ledger: String,
    pub pay_token: String,
    /// The decimals the capsule holds for that token. The caller already
    /// scaled the price with its own copy, so this is here to be checked
    /// against it: two components disagreeing about a token's decimals
    /// disagree about the price by a power of ten, silently.
    pub pay_token_decimals: u8,
    pub to: String,
    pub data: String,
    pub value: String,
    pub content_access_id: String,
    /// 0 free, 1 buy once, 2 buy and resell.
    pub op_type_code: u16,
    /// The resale cut in deci-percent, stated only by the one method that
    /// carries one. Null would read as "resale at no cut" rather than "resale
    /// is not on offer".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reseller_cut: Option<u16>,
    /// Always false: assembling a call is not signing one.
    pub signed: bool,
}

pub const PROTECTED_CONTENT_MINT_RECEIPT_SCHEMA_V1: &str =
    "elastos.chain.protected-content-mint-receipt/v1";

/// What a settled mint proved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtectedContentMintReceiptV1 {
    pub schema: String,
    pub network: String,
    pub chain_id: u64,
    pub token_id: String,
    /// The operative the mint produced, or the zero address for a free mint,
    /// which creates none.
    pub operative: String,
    /// The listing the mint created, carried here because the mint emits
    /// `ItemListed` in the same transaction.
    ///
    /// ABSENT for a free mint, which lists nothing. Stated as absent rather
    /// than as a zero price, which a reader could mistake for a sale at no
    /// cost -- and declared `Option` on both sides, which is the whole reason
    /// this type is shared: a free mint's null answer against a non-optional
    /// field is a refusal nobody sees until a free mint is tried.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pay_token: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The point of the type: what one side writes, the other side reads.
    ///
    /// Both directions, because `deny_unknown_fields` makes an extra field as
    /// fatal as a missing one, and a serialize-only test would miss it.
    #[test]
    fn creator_mint_answers_round_trip() {
        let source = ProtectedContentCreatorMintSourceV1 {
            schema: PROTECTED_CONTENT_CREATOR_MINT_SOURCE_SCHEMA_V1.to_string(),
            network: "base-mainnet".to_string(),
            chain_namespace: "eip155:8453".to_string(),
            pay_tokens: vec![ProtectedContentPayTokenV1 {
                symbol: "USDC".to_string(),
                address: "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913".to_string(),
                decimals: 6,
            }],
            abi: "elacity_mint_v1".to_string(),
            function: "mint(string,uint16,bytes,bytes)".to_string(),
            authority_gateway_contract: None,
        };
        let encoded = serde_json::to_string(&source).unwrap();
        assert_eq!(
            serde_json::from_str::<ProtectedContentCreatorMintSourceV1>(&encoded).unwrap(),
            source
        );
        // Absent rather than null, so a reader is not told "no authority" by a
        // field that simply was not configured.
        assert!(!encoded.contains("authority_gateway_contract"));

        let mint = ProtectedContentCreatorMintV1 {
            schema: PROTECTED_CONTENT_CREATOR_MINT_SCHEMA_V1.to_string(),
            network: "base-mainnet".to_string(),
            chain_namespace: "eip155:8453".to_string(),
            function: "mint(string,uint16,bytes,bytes)".to_string(),
            ledger: "0x0ebac909d31ef0074495e752c0cf4ea49ba13c41".to_string(),
            pay_token: "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913".to_string(),
            pay_token_decimals: 6,
            to: "0x0ebac909d31ef0074495e752c0cf4ea49ba13c41".to_string(),
            data: "0xdeadbeef".to_string(),
            value: "0x0".to_string(),
            content_access_id: "0x41414141414141414141414141414141".to_string(),
            op_type_code: 1,
            reseller_cut: None,
            signed: false,
        };
        let encoded = serde_json::to_string(&mint).unwrap();
        assert_eq!(
            serde_json::from_str::<ProtectedContentCreatorMintV1>(&encoded).unwrap(),
            mint
        );
        assert!(!encoded.contains("reseller_cut"));

        // A free mint lists nothing, and that absence has to survive the trip.
        let free = ProtectedContentMintReceiptV1 {
            schema: PROTECTED_CONTENT_MINT_RECEIPT_SCHEMA_V1.to_string(),
            network: "base-mainnet".to_string(),
            chain_id: 8453,
            token_id: "0x3".to_string(),
            operative: "0x0000000000000000000000000000000000000000".to_string(),
            quantity: None,
            price: None,
            pay_token: None,
        };
        let encoded = serde_json::to_string(&free).unwrap();
        assert_eq!(
            serde_json::from_str::<ProtectedContentMintReceiptV1>(&encoded).unwrap(),
            free
        );

        // And an explicit null reads as absent too, which is what a capsule
        // that states its nulls actually sends.
        let with_nulls = serde_json::json!({
            "schema": PROTECTED_CONTENT_MINT_RECEIPT_SCHEMA_V1,
            "network": "base-mainnet",
            "chain_id": 8453,
            "token_id": "0x3",
            "operative": "0x0000000000000000000000000000000000000000",
            "quantity": serde_json::Value::Null,
            "price": serde_json::Value::Null,
            "pay_token": serde_json::Value::Null,
        });
        assert_eq!(
            serde_json::from_value::<ProtectedContentMintReceiptV1>(with_nulls).unwrap(),
            free
        );
    }
}
