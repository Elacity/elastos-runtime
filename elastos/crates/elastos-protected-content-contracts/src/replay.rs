use thiserror::Error;

use crate::{Digest32, ReplayNonce16};

/// The exact authority scope and request nonce claimed by a request.
///
/// Scope hashes are domain-separated canonical hashes. Request times and other
/// non-authority fields are intentionally outside the scope, so mutating them
/// cannot make reuse of the same authority nonce acceptable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ReplayClaimKeyV1 {
    authority_scope_hash: Digest32,
    nonce: ReplayNonce16,
}

impl ReplayClaimKeyV1 {
    pub const fn new(authority_scope_hash: Digest32, nonce: ReplayNonce16) -> Self {
        Self {
            authority_scope_hash,
            nonce,
        }
    }

    pub const fn authority_scope_hash(&self) -> Digest32 {
        self.authority_scope_hash
    }

    pub const fn nonce(&self) -> ReplayNonce16 {
        self.nonce
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ReplayClaimError {
    #[error("authority nonce was already claimed")]
    AlreadyClaimed,
    #[error("atomic replay storage is unavailable")]
    Unavailable,
}

/// Runtime/node integration must implement atomic insert-if-absent semantics.
///
/// A successful call durably owns `key` until at least `expires_at`. Concurrent
/// calls for the same key must have at most one success. Returning success
/// before durable ownership exists violates this contract. This crate exposes
/// no production in-memory implementation.
pub trait AtomicReplayClaimer {
    fn claim(
        &mut self,
        key: ReplayClaimKeyV1,
        expires_at: u64,
        now: u64,
    ) -> Result<(), ReplayClaimError>;
}
