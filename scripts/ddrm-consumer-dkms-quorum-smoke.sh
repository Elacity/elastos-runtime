#!/usr/bin/env bash
#
# dDRM consumer-half smoke against a REAL 2-of-3 QUORUM dKMS authority (Day 113–116).
#
# A thin sibling of `ddrm-consumer-dkms-threshold-smoke.sh`: same end-to-end open, but the
# runtime provisions THREE secret-holding dKMS nodes and SHAMIR-splits the content CEK over
# GF(256) into three INDEXED shares (`x ‖ p(x)`, the coordinate sealed + authenticated INSIDE
# each escrow). ANY TWO live nodes serve an open — the production `DrmHost` run-path drives
# real node-kill failover:
#
#     drm/open -> rights -> key (quorum release: any 2 of 3) -> decrypt (Lagrange combine in-VM)
#
# Verify mode drives the quorum gates live: node C killed -> the open SURVIVES (A+B serve);
# two nodes dead -> BELOW quorum -> fails closed, no record; node B killed -> A+C serve (the
# x=1/x=3 Lagrange pair); a MIS-INDEXED share (a genuine node re-sealing a payload that claims
# another node's coordinate) and a DUPLICATED share (one node's view twice) both fail closed at
# the decrypt boundary.
#
# This is the availability property PC2 rents from Lit's opaque t-of-n BLS network
# (`decryptAndCombine`, non-media-decrypt.js:76) — and PC2's CURRENT path (Chipotle) gave it
# up entirely for a single master PKP in one TEE (chipotle-client.ts:1290). Here t, n, the
# field arithmetic, the quorum policy, and the failover are explicit, owned, and gated.
#
# Usage:  scripts/ddrm-consumer-dkms-quorum-smoke.sh
# Exit:   0 on PASS, 1 on FAIL.

set -uo pipefail
exec "$(cd "$(dirname "$0")" && pwd)/ddrm-consumer-smoke.sh" --threshold --nodes 3
