#!/usr/bin/env bash
#
# dDRM consumer-half smoke against the EXTERNAL `dkms` authority (Day 85–86).
#
# A thin sibling of `ddrm-consumer-smoke.sh`: it runs the SAME end-to-end open, but with
# `authority.backend = dkms`. The publish phase PROVISIONS an immutable external-authority
# descriptor (the dKMS-node analogue: master key material + published-identity pins); the
# open RESOLVES the authority's stable identity from that descriptor, recovers the
# publish-time escrow, decrypts the segment, and PROVES the descriptor was read-only
# (immutable published data) across the open. Switching from `reference` to `dkms` is a
# ONE-FIELD config change — the open path is byte-identical — so this proves the backend
# swap is invisible to the open (PC2's getSessionView backend dispatch, downstream agnostic).
#
# Usage:  scripts/ddrm-consumer-dkms-smoke.sh
# Exit:   0 on PASS, 1 on FAIL.

set -uo pipefail
exec "$(cd "$(dirname "$0")" && pwd)/ddrm-consumer-smoke.sh" --backend dkms
