#!/usr/bin/env python3
"""
Offline validator for an `elastos.dkms.authority/v2` descriptor.

This mirrors — exactly, by hand — the invariants the REAL runtime parser enforces in
`capsules/key-provider/src/main.rs::build_dkms_client` (+ the threshold block parser) and the
node-set pinning in `scripts/dev/ddrm-runtime-open/src/main.rs`. It invents no fields: a
descriptor that passes here is shaped the way the runtime will accept it, and a descriptor that
fails here would fail closed at runtime.

What it checks:
  * schema == "elastos.dkms.authority/v2"
  * NO secret ever appears (an `authority_master_seed_b64`, the rejected v1 shape, is a HARD fail)
  * top-level node identity present, base64-decodable, with a well-formed endpoint
  * if a `threshold` block exists: t == 2 with EXACTLY 2 or 3 nodes; nodes[0] matches the
    top-level identity; every node carries the 3 public fields; ALL verifying keys are DISTINCT
    (a duplicated identity silently collapses the threshold)

Usage:
  dkms-validate-descriptor.py PATH [--require-tcp]

Exit: 0 = valid, 1 = invalid, 2 = usage error.
"""
import base64
import json
import re
import sys

SCHEMA = "elastos.dkms.authority/v2"
SECRET_KEYS = {"authority_master_seed_b64", "master_seed_b64", "seal_seed_b64", "recipient_secret_b64"}
ENDPOINT_TCP = re.compile(r"^tcp:[^:]+:\d{1,5}$")

errors = []
warnings = []


def err(msg):
    errors.append(msg)


def warn(msg):
    warnings.append(msg)


def is_b64(s):
    if not isinstance(s, str) or not s.strip():
        return False
    try:
        base64.b64decode(s, validate=True)
        return True
    except Exception:
        return False


def scan_for_secrets(obj, path="$"):
    if isinstance(obj, dict):
        for k, v in obj.items():
            if k in SECRET_KEYS:
                err(f"SECRET FIELD '{k}' at {path} — a descriptor MUST be public-only; the "
                    f"recovery secret must never reach the runtime (rejected by the parser)")
            scan_for_secrets(v, f"{path}.{k}")
    elif isinstance(obj, list):
        for i, v in enumerate(obj):
            scan_for_secrets(v, f"{path}[{i}]")


def check_endpoint(ep, where, require_tcp):
    if not isinstance(ep, str) or not ep.strip():
        err(f"{where}: authority_endpoint is missing or empty")
        return
    if ep.startswith("tcp:"):
        if not ENDPOINT_TCP.match(ep):
            err(f"{where}: authority_endpoint '{ep}' is not a well-formed tcp:HOST:PORT")
    else:
        if not ep.startswith("/"):
            err(f"{where}: authority_endpoint '{ep}' is neither tcp:HOST:PORT nor an absolute "
                f"unix socket path")
        if require_tcp:
            err(f"{where}: --require-tcp set but endpoint '{ep}' is a unix path (a remote node "
                f"must publish a tcp: endpoint)")


def check_node(node, where, require_tcp):
    if not isinstance(node, dict):
        err(f"{where}: not an object")
        return
    for field in ("verifying_key_b64", "recipient_pub_b64"):
        if not is_b64(node.get(field)):
            err(f"{where}: {field} is missing or not valid base64")
    check_endpoint(node.get("authority_endpoint"), where, require_tcp)


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    flags = {a for a in sys.argv[1:] if a.startswith("--")}
    if len(args) != 1:
        print("usage: dkms-validate-descriptor.py PATH [--require-tcp]", file=sys.stderr)
        return 2
    require_tcp = "--require-tcp" in flags
    path = args[0]

    try:
        with open(path, "rb") as f:
            desc = json.load(f)
    except FileNotFoundError:
        print(f"INVALID: descriptor {path} not found", file=sys.stderr)
        return 1
    except json.JSONDecodeError as e:
        print(f"INVALID: descriptor {path} is not valid JSON: {e}", file=sys.stderr)
        return 1

    if not isinstance(desc, dict):
        print("INVALID: descriptor root is not a JSON object", file=sys.stderr)
        return 1

    scan_for_secrets(desc)

    if desc.get("schema") != SCHEMA:
        err(f"schema is {desc.get('schema')!r}, expected {SCHEMA!r}")

    check_node(desc, "top-level", require_tcp)

    threshold = desc.get("threshold")
    if threshold is None:
        warn("no `threshold` block — this is a SINGLE-NODE descriptor (legacy rail, no quorum). "
             "For a 2-of-3 deployment, add a threshold block with t:2 and three nodes.")
    else:
        t = threshold.get("t")
        nodes = threshold.get("nodes")
        if t != 2:
            err(f"threshold.t is {t!r}; the runtime accepts EXACTLY t == 2 (2-of-2 or 2-of-3)")
        if not isinstance(nodes, list) or len(nodes) not in (2, 3):
            err("threshold.nodes must be a list of EXACTLY 2 or 3 nodes")
        else:
            for i, n in enumerate(nodes):
                check_node(n, f"threshold.nodes[{i}]", require_tcp)
            n0 = nodes[0]
            if isinstance(n0, dict):
                if n0.get("verifying_key_b64") != desc.get("verifying_key_b64") or \
                   n0.get("recipient_pub_b64") != desc.get("recipient_pub_b64"):
                    err("threshold.nodes[0] does not match the top-level node identity "
                        "(they must be one coherent identity)")
            vks = [n.get("verifying_key_b64") for n in nodes if isinstance(n, dict)]
            if len(set(vks)) != len(vks):
                err("threshold nodes share a verifying key — a t-of-n split needs DISTINCT "
                    "secret-holding nodes (a duplicate silently collapses the threshold)")
            endpoints = [n.get("authority_endpoint") for n in nodes if isinstance(n, dict)]
            if len(set(endpoints)) != len(endpoints):
                warn("two nodes advertise the SAME endpoint — confirm these are genuinely "
                     "distinct hosts (distinct identities can still share a host in a lab, but "
                     "in production each node should be a separate failure domain)")

    for w in warnings:
        print(f"WARN:  {w}")
    if errors:
        for e in errors:
            print(f"ERROR: {e}", file=sys.stderr)
        print(f"\nINVALID: {path} ({len(errors)} error(s))", file=sys.stderr)
        return 1
    n = len(threshold["nodes"]) if isinstance(threshold, dict) and isinstance(threshold.get("nodes"), list) else 1
    shape = f"2-of-{n}" if threshold else "single-node"
    print(f"\nVALID: {path} — {shape} {SCHEMA}, public-only, runtime will accept it")
    return 0


if __name__ == "__main__":
    sys.exit(main())
