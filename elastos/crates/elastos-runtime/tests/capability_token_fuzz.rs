//! Bounded robustness fuzz for the capability-token parser.
//!
//! `CapabilityToken::from_base64`/`from_bytes` is an attacker-facing trust-boundary parser:
//! `carrier_bridge` deserializes a guest-supplied token string before validation. Such a
//! parser must NEVER panic on malformed input — it must return `Err`. This throws a large
//! deterministic corpus (structured edge cases + pseudo-random bytes) at both entries and
//! asserts no panic. Deterministic seed ⇒ any failure is reproducible. Runs under
//! `just verify`. (We found a real DoS this way in the WebAuthn parser; this guards the
//! capability parser the same way.)

use std::panic::{catch_unwind, AssertUnwindSafe};

use elastos_runtime::capability::CapabilityToken;

/// xorshift64* — deterministic, dependency-free PRNG so failures reproduce exactly.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn bytes(&mut self, max_len: usize) -> Vec<u8> {
        let len = (self.next() as usize) % (max_len + 1);
        (0..len).map(|_| (self.next() & 0xff) as u8).collect()
    }
    /// A random string over the base64url alphabet + occasional junk, to exercise both the
    /// base64-decode reject path and the deeper deserialize path on bytes that DO decode.
    fn b64ish(&mut self, max_len: usize) -> String {
        const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_=!. ";
        let len = (self.next() as usize) % (max_len + 1);
        (0..len)
            .map(|_| A[(self.next() as usize) % A.len()] as char)
            .collect()
    }
}

fn no_panic_from_bytes(input: &[u8]) {
    let r = catch_unwind(AssertUnwindSafe(|| {
        let _ = CapabilityToken::from_bytes(input);
    }));
    assert!(
        r.is_ok(),
        "CapabilityToken::from_bytes PANICKED on {} bytes: {:02x?}",
        input.len(),
        input
    );
}

fn no_panic_from_base64(input: &str) {
    let r = catch_unwind(AssertUnwindSafe(|| {
        let _ = CapabilityToken::from_base64(input);
    }));
    assert!(
        r.is_ok(),
        "CapabilityToken::from_base64 PANICKED on {:?}",
        input
    );
}

#[test]
fn from_bytes_never_panics() {
    // Structured edge cases: empty, single byte, all-zero/all-0xff at common boundary sizes
    // (the token layout has fixed-size + length-prefixed fields — boundaries are where slicing bugs live).
    no_panic_from_bytes(&[]);
    for &n in &[1usize, 15, 16, 17, 63, 64, 65, 127, 128, 255, 256] {
        no_panic_from_bytes(&vec![0u8; n]);
        no_panic_from_bytes(&vec![0xffu8; n]);
        // a length-prefix-looking blob: a huge declared length (8x 0xff) then zero-padding
        let mut blob = vec![0xffu8; 8];
        blob.resize(n.max(8), 0u8);
        no_panic_from_bytes(&blob);
    }

    // Pseudo-random fuzz.
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..30_000 {
        let raw = rng.bytes(1024);
        no_panic_from_bytes(&raw);
    }
}

#[test]
fn from_base64_never_panics() {
    for s in ["", "=", "A", "AA", "AAA", "AAAA", "////", "!!!!", "-_-_"] {
        no_panic_from_base64(s);
    }
    no_panic_from_base64(&"A".repeat(200_000)); // oversized

    let mut rng = Rng(0x1234_5678_9ABC_DEF0);
    for _ in 0..30_000 {
        let s = rng.b64ish(1024);
        no_panic_from_base64(&s);
    }
}
