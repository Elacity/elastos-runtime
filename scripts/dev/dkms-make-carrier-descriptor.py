#!/usr/bin/env python3
"""Derive a Carrier (did:key) dKMS descriptor from the existing tcp/WireGuard one.

The dKMS quorum descriptor pins each node's PUBLISHED PQ identity (verifying_key_b64 +
recipient_pub_b64) and an `authority_endpoint` (where to reach it). Migrating transport to
Carrier changes ONLY the endpoint: `tcp:10.66.66.x:9443` -> `carrier:did:key:z6Mk...`, where
the did is what that node's `dkms-carrier-node` bridge prints on boot. The PQ pins are
untouched, so the end-to-end channel + escrow + authorization are byte-for-byte identical.

Usage:
  dkms-make-carrier-descriptor.py <in_tcp_v2.json> <out_carrier.json> <did0> [<did1> <did2> ...]

The dids are applied IN ORDER to threshold.nodes[] (node0=A, node1=B, node2=C), matching the
descriptor's node ordering. The top-level endpoint mirrors node0.
"""
import json
import sys


def main() -> int:
    if len(sys.argv) < 4:
        print(__doc__)
        return 2
    in_path, out_path = sys.argv[1], sys.argv[2]
    dids = [d.strip() for d in sys.argv[3:] if d.strip()]

    desc = json.load(open(in_path))
    nodes = desc.get("threshold", {}).get("nodes", [])
    if not nodes:
        # single-node descriptor: just rewrite the top-level endpoint.
        if not dids:
            print("error: no dids provided", file=sys.stderr)
            return 1
        desc["authority_endpoint"] = carrier(dids[0])
        json.dump(desc, open(out_path, "w"), indent=2)
        print(f"wrote {out_path} (single node -> {carrier(dids[0])})")
        return 0

    if len(dids) != len(nodes):
        print(
            f"error: descriptor has {len(nodes)} nodes but {len(dids)} dids were given",
            file=sys.stderr,
        )
        return 1

    for node, did in zip(nodes, dids):
        node["authority_endpoint"] = carrier(did)
    # Top-level pins mirror node0 (the descriptor invariant).
    desc["authority_endpoint"] = carrier(dids[0])

    json.dump(desc, open(out_path, "w"), indent=2)
    print(f"wrote {out_path}:")
    for i, did in enumerate(dids):
        print(f"  node{i}: {carrier(did)}")
    return 0


def carrier(did: str) -> str:
    did = did.strip()
    if did.startswith("carrier:"):
        return did
    return f"carrier:{did}"


if __name__ == "__main__":
    raise SystemExit(main())
