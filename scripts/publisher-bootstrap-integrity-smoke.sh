#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

python3 - <<'PY'
from pathlib import Path

install = Path("scripts/install.sh").read_text()
publish = Path("scripts/publish-release.sh").read_text()

required_install = [
    'data.get("role") != "publisher"',
    "validate_explicit_source_bootstrap_pair",
    "trusted-source Carrier bootstrap overrides are atomic",
]
required_publish = [
    "/.well-known/elastos/carrier-bootstrap.json?role=publisher",
    'bootstrap.get("role") != "publisher"',
    "trusted-source Carrier bootstrap requires both ELASTOS_SOURCE_CONNECT_TICKET and ELASTOS_PUBLISHER_NODE_ID",
]

for needle in required_install:
    if needle not in install:
        raise SystemExit(f"[publisher-bootstrap-integrity] install.sh missing {needle!r}")

for needle in required_publish:
    if needle not in publish:
        raise SystemExit(f"[publisher-bootstrap-integrity] publish-release.sh missing {needle!r}")

if "keys node-id" in publish:
    raise SystemExit("[publisher-bootstrap-integrity] publish-release.sh must not pair a ticket with keys node-id fallback")


def parse_bootstrap(data):
    if data.get("schema") != "elastos.carrier.bootstrap/v1":
        raise ValueError("bad schema")
    if data.get("role") != "publisher":
        raise ValueError("bad role")
    ticket = (data.get("ticket") or "").strip()
    node_id = (data.get("node_id") or "").strip()
    if not ticket or not node_id:
        raise ValueError("incomplete pair")
    return ticket, node_id


assert parse_bootstrap({
    "schema": "elastos.carrier.bootstrap/v1",
    "role": "publisher",
    "ticket": "ticket",
    "node_id": "node",
}) == ("ticket", "node")

for bad in (
    {"schema": "elastos.carrier.bootstrap/v1", "role": "runtime", "ticket": "ticket", "node_id": "node"},
    {"schema": "elastos.carrier.bootstrap/v1", "role": "publisher", "ticket": "ticket", "node_id": ""},
    {"schema": "elastos.carrier.bootstrap/v1", "role": "publisher", "ticket": "", "node_id": "node"},
):
    try:
        parse_bootstrap(bad)
    except ValueError:
        pass
    else:
        raise SystemExit(f"[publisher-bootstrap-integrity] accepted invalid bootstrap: {bad!r}")

print("[publisher-bootstrap-integrity] PASS")
PY
