//! Fuzz harness for the Carrier-bridge framing parser.
//!
//! **Phase 10 Day 4-8.** Closes the `MAC_THREAT_MODEL.md` TB3 "NOT
//! YET FUZZ-TESTED" gap.
//!
//! Target: [`elastos_server::carrier_bridge::parse_carrier_line`] —
//! the pure-function extraction of the framing + JSON-parse logic
//! used by both microVM and WASM Carrier bridges.
//!
//! Invariants this harness asserts on every input:
//!   1. `parse_carrier_line` never panics.
//!   2. For inputs exceeding `CARRIER_MAX_LINE_BYTES`, the function
//!      short-circuits with `Err(LineTooLarge)` without attempting a
//!      UTF-8 decode or JSON parse on the oversized buffer.
//!   3. A successful parse always returns either `Ok(None)` (empty /
//!      whitespace-only) or `Ok(Some(Value))` where `Value` is a
//!      well-formed `serde_json::Value`.
//!
//! Operator usage:
//!   cargo +nightly fuzz run carrier_bridge_framing
//!   cargo +nightly fuzz run carrier_bridge_framing -- -max_total_time=600
//!
//! The seed corpus at `corpus/carrier_bridge_framing/` includes
//! known-good envelopes captured from a managed-Home session plus
//! adversarial edge cases (oversized, malformed JSON, truncated
//! UTF-8, deeply nested arrays). The dictionary at
//! `dict/carrier_bridge_framing.dict` biases mutations toward
//! syntactically plausible envelopes.

#![no_main]

use elastos_server::carrier_bridge::{
    parse_carrier_line, CarrierFrameError, CARRIER_MAX_LINE_BYTES,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let result = parse_carrier_line(data);

    // Cross-check the LineTooLarge short-circuit: if the input
    // exceeds the cap, the function MUST have returned LineTooLarge
    // and reported the original length. Any other outcome (Ok, or a
    // different Err variant) on oversized input is a finding.
    if data.len() > CARRIER_MAX_LINE_BYTES {
        match result {
            Err(CarrierFrameError::LineTooLarge { len }) => {
                assert_eq!(
                    len,
                    data.len(),
                    "LineTooLarge.len must mirror the input length"
                );
            }
            other => panic!(
                "oversized input ({} bytes) must yield LineTooLarge, got {:?}",
                data.len(),
                other
            ),
        }
    }
});
