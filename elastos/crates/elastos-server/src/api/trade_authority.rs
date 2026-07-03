//! Gateway-side SECONDARY-MARKET (access-token resale) calldata assembly: list / withdraw access for resale.
//!
//! The inverse-discipline twin of `buy_authority`: PURE — no RPC, no keys, no signing. It builds the
//! UNSIGNED `eth_sendTransaction`-shaped calldata for the ERC-1155 access-token resale flow and hands it
//! to `wallet-provider` for a human-approved signature (Principle 16). Selectors are keccak-pinned and were
//! confirmed PRESENT in the deployed Base AuthorityGateway / Operative bytecode (docs/marketplace/CONTRACTS.md
//! and verify-selectors.mjs). The caller must FIRST re-verify ownership live (`hasAccessByContentId`) and resolve
//! the real ledger `tokenId` — same Phase-1 discipline as buy.
//!
//! Verified signatures (keccak256 4-byte selector) — ARG SEMANTICS GROUNDED IN elacity-web v3 (MediaContext.tsx):
//!   sellAccess(address,uint256,uint256,uint256,address)  -> 0x9a3fa9f5   (LEDGER, tokenId, quantity, pricePerToken, payToken)
//!   withdrawListing(address,uint256,uint256)             -> 0x3e65bbba   (OPERATIVE, tokenId, quantity)   <- arg0 differs from sellAccess
//!   setApprovalForAll(address,bool)                      -> 0xa22cb465   (operator=GATEWAY, approved)     <- SENT TO the OPERATIVE ERC-1155
//!
//! ASYMMETRY (real, from v3): `sellAccess` arg0 = **ledger** (DIGITAL_ASSET_LEDGER); `withdrawListing` arg0 =
//! **operative** — the gateway maps ledger<->operative internally and listings are keyed by operative. The
//! access-token resale `tokenId` is the ERC-1155 ACCESS_TOKEN **role id** (commonly `1`), distinct from
//! `buyAccess`'s media tokenId — confirm against a real listing before mainnet. The ERC-1155 approval is
//! `setApprovalForAll(operator = AuthorityGateway, true)` sent to the **Operative** contract, guarded by
//! `isApprovedForAll(account, gateway)`. `resellerCut` is NOT an arg — the chain reads it from the Operative's
//! stored config. Royalty-share resale (tokenId = ROYALTY_SHARE = 2) is a SEPARATE TradeGateway path
//! (`sellToken`/`createOffer`/`buyToken`), not handled here.
//!
//! Standalone-verifiable: std-only, so `rustc --test` runs the encoding unit tests without a workspace build.
//! Integration (wire into gateway.rs as POST /api/market/order/{sell,withdraw} + the approval leg; broadcast via
//! `chain-provider.broadcast_transaction` after the wallet signs) is a follow-up. NOT live-chain tested.

pub const SEL_SELL_ACCESS: &str = "9a3fa9f5";
pub const SEL_WITHDRAW_LISTING: &str = "3e65bbba";
pub const SEL_SET_APPROVAL_FOR_ALL: &str = "a22cb465";

/// An UNSIGNED transaction: target + calldata only. Carries no secret, no signer, no nonce/gas
/// (sourced by `chain-provider.prepare_transaction` at sign time). `value` is "0" — resale is ERC-20 priced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsignedTx {
    pub to: String,
    pub data: String,
    pub value: String,
}

/// Normalize a 20-byte hex address to a left-padded 32-byte ABI word (lowercase, no 0x).
fn word_addr(addr: &str) -> Result<String, String> {
    let clean = addr.strip_prefix("0x").unwrap_or(addr).to_lowercase();
    if clean.len() != 40 || !clean.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("invalid 20-byte address: {addr}"));
    }
    Ok(format!("{:0>64}", clean))
}

/// A u128 (quantity / price in minor units) as a 32-byte ABI word.
fn word_u128(value: u128) -> String {
    format!("{value:064x}")
}

/// A pre-validated big integer (e.g. a 256-bit tokenId) given as hex (no 0x) -> 32-byte word.
fn word_u256_hex(hex: &str) -> Result<String, String> {
    let clean = hex.strip_prefix("0x").unwrap_or(hex).to_lowercase();
    if clean.is_empty() || clean.len() > 64 || !clean.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("invalid uint256 hex: {hex}"));
    }
    Ok(format!("{clean:0>64}"))
}

fn word_bool(b: bool) -> String {
    word_u128(if b { 1 } else { 0 })
}

/// `sellAccess(ledger, tokenId, quantity, pricePerToken, payToken)` — list owned access for resale.
/// **arg0 = the LEDGER (DIGITAL_ASSET_LEDGER)**; `token_id_hex` = the access-token id (hex, no 0x; the ERC-1155
/// ACCESS_TOKEN role id, commonly `1`); `price_per_token`/`quantity` in token minor units (USDC = 6 decimals);
/// `pay_token` = canonical Base USDC `0x833589fC…`.
pub fn build_sell_access(
    gateway: &str,
    ledger: &str,
    token_id_hex: &str,
    quantity: u128,
    price_per_token: u128,
    pay_token: &str,
) -> Result<UnsignedTx, String> {
    let data = format!(
        "0x{sel}{ledger}{token}{qty}{price}{pay}",
        sel = SEL_SELL_ACCESS,
        ledger = word_addr(ledger)?,
        token = word_u256_hex(token_id_hex)?,
        qty = word_u128(quantity),
        price = word_u128(price_per_token),
        pay = word_addr(pay_token)?,
    );
    Ok(UnsignedTx {
        to: gateway.to_string(),
        data,
        value: "0".to_string(),
    })
}

/// `withdrawListing(operative, tokenId, quantity)` — cancel a resale listing. The access right is unaffected.
/// **arg0 = the OPERATIVE** (NOT the ledger — listings are keyed by operative; this is the asymmetry vs sellAccess).
pub fn build_withdraw_listing(
    gateway: &str,
    operative: &str,
    token_id_hex: &str,
    quantity: u128,
) -> Result<UnsignedTx, String> {
    let data = format!(
        "0x{sel}{operative}{token}{qty}",
        sel = SEL_WITHDRAW_LISTING,
        operative = word_addr(operative)?,
        token = word_u256_hex(token_id_hex)?,
        qty = word_u128(quantity),
    );
    Ok(UnsignedTx {
        to: gateway.to_string(),
        data,
        value: "0".to_string(),
    })
}

/// `setApprovalForAll(operator, approved)` — the PRE step: approve the gateway to move the seller's ERC-1155
/// access tokens. **Sent to the OPERATIVE (ERC-1155) contract** (`to = operative`); `operator = AuthorityGateway`.
/// Guard with `isApprovedForAll(account, gateway)` and only emit when not already approved.
pub fn build_set_approval_for_all(
    operative: &str,
    operator_gateway: &str,
    approved: bool,
) -> Result<UnsignedTx, String> {
    let data = format!(
        "0x{sel}{op}{flag}",
        sel = SEL_SET_APPROVAL_FOR_ALL,
        op = word_addr(operator_gateway)?,
        flag = word_bool(approved),
    );
    Ok(UnsignedTx {
        to: operative.to_string(),
        data,
        value: "0".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GW: &str = "0x09dBe796f40ECEffEAccf243c3d758C4c1d8D87D";
    const LEDGER: &str = "0x6756e1407164ae34f8df5334d48d0e45c094b8b9";
    const OPERATIVE: &str = "0x483adcf310d9344cc017536810d65a87ebcc1760";
    const USDC: &str = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913";

    #[test]
    fn sell_access_arg0_is_ledger_then_four_words() {
        // ACCESS_TOKEN role id = 1, quantity = 2, price = 10_000 (0.01 USDC * 1e6)
        let tx = build_sell_access(GW, LEDGER, "1", 2, 10_000, USDC).unwrap();
        assert_eq!(tx.to.to_lowercase(), GW.to_lowercase());
        assert_eq!(tx.value, "0");
        let body = tx.data.strip_prefix("0x").unwrap();
        assert_eq!(&body[..8], SEL_SELL_ACCESS);
        assert_eq!(body.len(), 8 + 5 * 64);
        assert_eq!(
            &body[8..72],
            &format!("{:0>64}", &LEDGER[2..].to_lowercase())
        ); // arg0 = ledger
        assert_eq!(&body[72..136], &format!("{:064x}", 1u128)); // ACCESS_TOKEN tokenId
        assert_eq!(&body[136..200], &format!("{:064x}", 2u128)); // quantity
        assert_eq!(&body[200..264], &format!("{:064x}", 10_000u128)); // pricePerToken
        assert_eq!(
            &body[264..328],
            &format!("{:0>64}", &USDC[2..].to_lowercase())
        ); // payToken
    }

    #[test]
    fn withdraw_listing_arg0_is_operative_not_ledger() {
        let tx = build_withdraw_listing(GW, OPERATIVE, "1", 3).unwrap();
        let body = tx.data.strip_prefix("0x").unwrap();
        assert_eq!(&body[..8], SEL_WITHDRAW_LISTING);
        assert_eq!(body.len(), 8 + 3 * 64);
        assert_eq!(
            &body[8..72],
            &format!("{:0>64}", &OPERATIVE[2..].to_lowercase())
        ); // arg0 = OPERATIVE (the asymmetry)
        assert_ne!(
            &body[8..72],
            &format!("{:0>64}", &LEDGER[2..].to_lowercase())
        ); // NOT the ledger
        assert_eq!(&body[72..136], &format!("{:064x}", 1u128)); // tokenId
        assert_eq!(&body[136..200], &format!("{:064x}", 3u128)); // quantity
    }

    #[test]
    fn set_approval_sent_to_operative_operator_is_gateway() {
        let tx = build_set_approval_for_all(OPERATIVE, GW, true).unwrap();
        assert_eq!(tx.to.to_lowercase(), OPERATIVE.to_lowercase()); // sent to the Operative ERC-1155
        let body = tx.data.strip_prefix("0x").unwrap();
        assert_eq!(&body[..8], SEL_SET_APPROVAL_FOR_ALL);
        assert_eq!(body.len(), 8 + 2 * 64);
        assert_eq!(&body[8..72], &format!("{:0>64}", &GW[2..].to_lowercase())); // operator = gateway
        assert_eq!(&body[72..136], &format!("{:064x}", 1u128)); // approved = true
    }

    #[test]
    fn rejects_malformed_address_and_tokenid() {
        assert!(build_sell_access(GW, "0xnothex", "1", 1, 1, USDC).is_err());
        assert!(build_sell_access(GW, LEDGER, "0xZZ", 1, 1, USDC).is_err());
        assert!(build_sell_access(GW, LEDGER, &"f".repeat(65), 1, 1, USDC).is_err());
        // >256 bits
    }

    #[test]
    fn full_256bit_tokenid_round_trips() {
        let big = "f".repeat(64);
        let tx = build_withdraw_listing(GW, OPERATIVE, &big, 1).unwrap();
        assert_eq!(&tx.data.strip_prefix("0x").unwrap()[72..136], &big);
    }
}
