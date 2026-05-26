use super::*;
use bitcoin::consensus::encode::serialize;
use bitcoin::{hashes::Hash, sighash::SighashCache, Amount, EcdsaSighashType, Network, Witness};
use elastos_auth::ethereum_signed_message_hash;
use elastos_auth::normalize_evm_address;
use k256::ecdsa::SigningKey;
use sha3::{Digest, Keccak256};
use std::path::Path;

mod accounts;
mod approvals;
mod browser_signing;
mod proofs;
mod support;
