#!/usr/bin/env bash
#
# dDRM consumer-half smoke against a REAL 2-of-2 THRESHOLD dKMS authority (Day 99–100).
#
# A thin sibling of `ddrm-consumer-dkms-smoke.sh`: it runs the SAME end-to-end open, but the
# runtime provisions TWO secret-holding dKMS nodes (distinct stores/sockets/allow-lists),
# XOR-splits the content CEK at publish so each node escrows only ONE share (neither node ever
# holds the whole key), publishes a `threshold` descriptor (both nodes), and the production
# `DrmHost` run-path drives the FULL 2-of-2 release + decrypt:
#
#     drm/open -> rights -> key (dual-recover BOTH nodes) -> decrypt (unwrap BOTH shares + XOR in-VM)
#
# The CEK is reconstructed ONLY inside the decrypt boundary — it never exists whole in the
# key-provider. Verify mode additionally drives the adversarial gates: a release supplying only
# ONE share fails closed, and a 3-of-N threshold descriptor fails closed at init.
#
# This is the runtime's EXPLICIT, owned analogue of Lit's opaque `decryptAndCombine`
# (PC2 `non-media-decrypt.js:76`), where PC2 cannot inspect the share set / node membership and
# its own run-path stops at a single Lit RPC — the runtime drives two owned nodes end to end.
#
# Usage:  scripts/ddrm-consumer-dkms-threshold-smoke.sh
# Exit:   0 on PASS, 1 on FAIL.

set -uo pipefail
exec "$(cd "$(dirname "$0")" && pwd)/ddrm-consumer-smoke.sh" --threshold
