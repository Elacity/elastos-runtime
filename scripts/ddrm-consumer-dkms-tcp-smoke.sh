#!/usr/bin/env bash
#
# dDRM consumer-half smoke with the dKMS nodes OFF localhost (Day 105–108): the REAL 2-of-2
# THRESHOLD rail over a REAL NETWORK transport (TCP) with the app-layer ENCRYPTED,
# MUTUALLY-AUTHENTICATED channel.
#
# A thin sibling of `ddrm-consumer-dkms-threshold-smoke.sh`: the SAME end-to-end 2-of-2 open,
# but both secret-holding node daemons listen on real TCP endpoints (`tcp:127.0.0.1:PORT`,
# published in the descriptor) instead of Unix sockets, and the rail enforces the channel:
#
#   - at `hello`, each node publishes a master-derived channel KEM key ATTESTED under its
#     descriptor-pinned identity (a substituted key — an attacker terminating the TCP
#     connection — fails verification, fail-closed);
#   - every post-hello frame in BOTH directions is a SEALED envelope, AAD-bound to
#     (channel, direction, seq) — non-replayable, non-reflectable;
#   - a plaintext recover is REFUSED (`channel_required`); a plaintext downgrade after
#     establishment and a MITM-tampered sealed frame each DROP the connection;
#   - connect/read timeouts bound every network wait (a stalled node fails closed, no hang).
#
# Verify mode drives ALL the adversarial gates: the threshold gates (single share refused,
# 3-of-N refused, node-fault fail-closed, node-set swap/AAD/rotation) AND the network gates
# (28–31: plaintext recover, downgrade, MITM tamper, wrong channel key).
#
# Contrast PC2: its dDRM network boundary is HTTPS with `rejectUnauthorized: false`
# (`chipotle-client.ts:840`) — TLS verification is OFF and the channel authenticates nothing.
#
# Usage:  scripts/ddrm-consumer-dkms-tcp-smoke.sh
# Exit:   0 on PASS, 1 on FAIL.

set -uo pipefail
exec "$(cd "$(dirname "$0")" && pwd)/ddrm-consumer-smoke.sh" --threshold --transport tcp
