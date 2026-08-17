//! Canonical hashing of invocation arguments (W2 consent binding).
//!
//! [`canonical_input_hash`] produces a deterministic SHA-256 hex digest of a JSON
//! value that is independent of object key order, so the gateway (which raises a
//! consent request) and the runtime (which will later re-hash the actual
//! invocation arguments at approve/redeem time) compute the SAME hash for the
//! same logical arguments. Sharing this ONE implementation across both sides is
//! the point: two copies could drift and spuriously deny a legitimate re-invoke.
//!
//! Stability contract:
//! - Object entries are sorted lexicographically by key (`BTreeMap`); array order
//!   is preserved; scalars are unchanged. This holds even if some crate enables
//!   `serde_json`'s `preserve_order` feature, because we rebuild the tree.
//! - Numbers are NOT coerced: `1` and `1.0` hash differently by design (relies on
//!   `serde_json` built WITHOUT `arbitrary_precision`, which this crate does not
//!   enable).
//! - Duplicate object keys are collapsed last-wins by `serde_json` at parse time
//!   before this function runs; the parse is authoritative and we do not attempt
//!   to detect duplicates afterwards.
//! - This is CONSTRUCTION-stability (same logical args -> same hash regardless of
//!   key order), NOT RFC-8785 cross-language canonical JSON. Sufficient while a
//!   single producer (the gateway) exists.

use std::collections::BTreeMap;

use serde_json::Value;
use sha2::{Digest, Sha256};

/// Rebuild a [`Value`] into a canonical tree: objects become key-sorted maps,
/// arrays keep their order, scalars are unchanged.
fn canonicalize(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted: BTreeMap<String, Value> = map
                .iter()
                .map(|(k, v)| (k.clone(), canonicalize(v)))
                .collect();
            // Collecting from a BTreeMap inserts keys in sorted order, so the
            // resulting map is key-sorted regardless of its backing type.
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(items) => Value::Array(items.iter().map(canonicalize).collect()),
        scalar => scalar.clone(),
    }
}

/// Deterministic lowercase SHA-256 hex digest of `input`, independent of object
/// key order. See the module docs for the stability contract.
pub fn canonical_input_hash(input: &Value) -> String {
    let canonical = canonicalize(input);
    // Compact serialization (no whitespace); serializing a `Value` cannot fail.
    let bytes = serde_json::to_string(&canonical).unwrap_or_default();
    hex::encode(Sha256::digest(bytes.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hash_is_independent_of_object_key_order() {
        let a = json!({"a": 1, "b": 2, "nested": {"x": true, "y": false}});
        let b = json!({"b": 2, "nested": {"y": false, "x": true}, "a": 1});
        assert_eq!(canonical_input_hash(&a), canonical_input_hash(&b));
    }

    #[test]
    fn hash_survives_serialize_parse_round_trip() {
        let v = json!({"target": "film-x", "scopes": ["read", "open"], "n": 3});
        let round: Value = serde_json::from_str(&serde_json::to_string(&v).unwrap()).unwrap();
        assert_eq!(canonical_input_hash(&v), canonical_input_hash(&round));
    }

    #[test]
    fn hash_preserves_array_order_and_distinguishes_numbers() {
        // Array order is meaningful.
        assert_ne!(
            canonical_input_hash(&json!([1, 2])),
            canonical_input_hash(&json!([2, 1]))
        );
        // 1 and 1.0 are intentionally distinct (no numeric coercion).
        assert_ne!(
            canonical_input_hash(&json!(1)),
            canonical_input_hash(&json!(1.0))
        );
        // Digest is a well-formed lowercase hex SHA-256.
        let h = canonical_input_hash(&json!({"k": "v"}));
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
