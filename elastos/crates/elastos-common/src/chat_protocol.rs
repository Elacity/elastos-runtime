//! Shared chat protocol logic — signing, verification gating, nick binding, dedup.
//!
//! This module owns the chat protocol contract. All chat surfaces (native,
//! WASM, microVM, agent) must use these functions for protocol decisions.
//! Transport-specific operations (calling DID sign/verify, gossip send/recv)
//! stay in each surface's own code.

use sha2::{Digest, Sha256};

/// SHA-256 of "sender_id:ts:content" — the canonical signing payload.
///
/// Used by send (sign this) and receive (verify against this).
/// Every chat surface must use this exact format.
pub fn signing_payload_hex(sender_id: &str, ts: u64, content: &str) -> String {
    let data = format!("{}:{}:{}", sender_id, ts, content);
    hex::encode(Sha256::digest(data.as_bytes()))
}

/// Dedup key for gossip messages.
///
/// Uses SHA-256 of (sender_id, sender_nick, ts, content) to detect
/// duplicate messages. The key includes nick to distinguish messages
/// from different sessions that share the same DID.
pub fn dedup_key(sender_id: &str, sender_nick: &str, ts: u64, content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(sender_id.as_bytes());
    hasher.update(sender_nick.as_bytes());
    hasher.update(ts.to_le_bytes());
    hasher.update(content.as_bytes());
    hex::encode(hasher.finalize())
}

/// Should this message be displayed/acted on?
///
/// Returns false if the message is unverified AND the sender is unknown.
/// Known senders (previously verified nick) are allowed through even if
/// this particular message fails verification (backward compat with older
/// clients that may not sign every message).
pub fn should_accept_message(verified: bool, is_known_sender: bool) -> bool {
    verified || is_known_sender
}

/// Should this message trigger peer attachment?
///
/// Only verified messages should cause the runtime to attach the sender
/// as a room peer. Unverified messages must not influence peer topology.
pub fn should_attach_peer(verified: bool) -> bool {
    verified
}

/// Should this sender's nick→DID binding be recorded?
///
/// Only verified messages should establish nick ownership. This prevents
/// TOFU poisoning where an attacker claims a nick before the real user.
pub fn should_record_nick(verified: bool) -> bool {
    verified
}

/// Check if a message is from the same chat instance (should be skipped as echo).
///
/// Requires both DID and session_id to match. On a shared runtime, multiple
/// capsules share the same DID but have different session IDs.
pub fn is_own_message(
    self_did: &str,
    self_session_id: &str,
    sender_did: &str,
    sender_session_id: Option<&str>,
) -> bool {
    if self_did.is_empty() || sender_did != self_did {
        return false;
    }
    match sender_session_id.filter(|s| !s.is_empty()) {
        Some(sid) => sid == self_session_id,
        None => true, // No session_id = assume same instance (legacy compat)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signing_payload_deterministic() {
        let h1 = signing_payload_hex("did:key:zAlice", 1000, "hello");
        let h2 = signing_payload_hex("did:key:zAlice", 1000, "hello");
        assert_eq!(h1, h2);
    }

    #[test]
    fn signing_payload_differs_on_any_field() {
        let base = signing_payload_hex("did:key:zAlice", 1000, "hello");
        assert_ne!(base, signing_payload_hex("did:key:zBob", 1000, "hello"));
        assert_ne!(base, signing_payload_hex("did:key:zAlice", 1001, "hello"));
        assert_ne!(base, signing_payload_hex("did:key:zAlice", 1000, "world"));
    }

    #[test]
    fn dedup_key_deterministic() {
        let k1 = dedup_key("did:key:zAlice", "alice", 1000, "hello");
        let k2 = dedup_key("did:key:zAlice", "alice", 1000, "hello");
        assert_eq!(k1, k2);
    }

    #[test]
    fn dedup_key_includes_nick() {
        let k1 = dedup_key("did:key:zShared", "alice", 1000, "hello");
        let k2 = dedup_key("did:key:zShared", "bob", 1000, "hello");
        assert_ne!(k1, k2, "different nicks must produce different dedup keys");
    }

    #[test]
    fn accept_message_gate() {
        assert!(
            should_accept_message(true, false),
            "verified unknown = accept"
        );
        assert!(should_accept_message(true, true), "verified known = accept");
        assert!(
            should_accept_message(false, true),
            "unverified known = accept"
        );
        assert!(
            !should_accept_message(false, false),
            "unverified unknown = reject"
        );
    }

    #[test]
    fn own_message_detection() {
        assert!(is_own_message("did:key:z1", "s1", "did:key:z1", Some("s1")));
        assert!(!is_own_message(
            "did:key:z1",
            "s1",
            "did:key:z1",
            Some("s2")
        ));
        assert!(!is_own_message(
            "did:key:z1",
            "s1",
            "did:key:z2",
            Some("s1")
        ));
        assert!(is_own_message("did:key:z1", "s1", "did:key:z1", None)); // legacy
        assert!(!is_own_message("", "s1", "did:key:z1", Some("s1"))); // empty self
    }

    #[test]
    fn attach_and_nick_require_verification() {
        assert!(should_attach_peer(true));
        assert!(!should_attach_peer(false));
        assert!(should_record_nick(true));
        assert!(!should_record_nick(false));
    }
}
