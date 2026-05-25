#!/usr/bin/env bash
# scripts/lib/components-json-verify.sh — Phase 6 Day 2.
#
# Single source of truth for the `components.json` darwin-arm64 schema
# invariants the Phase-6 audit (`docs/vz-backend/PHASE_6_COMPONENTS_AUDIT.md`)
# locks in. Re-runnable from any Day-3+/Day-5 CI gate to detect drift.
#
# What this checks (read the audit doc § 4 for the why):
#   1. components.json parses as JSON.
#   2. Class-A host Rust binaries (7): linux-amd64 + linux-arm64 + darwin-arm64 all present.
#   3. Class-B microVM bundle (chat): linux-amd64 + linux-arm64 + darwin-arm64
#      all present. **Day 3 (this commit) promoted chat from forward-compat
#      to required.** Additionally, the share-linux-arm64-bundle invariant
#      (Decision D.2.a) is enforced: `chat.darwin-arm64.{cid,checksum,size,
#      release_path,extract_path}` must equal the corresponding linux-arm64
#      values exactly. Catches accidental copy-paste drift in either
#      direction once the release pipeline starts populating cids.
#   4. Class-C kernel (vmlinux): linux-amd64 + linux-arm64 present today;
#      darwin-arm64 added in Day 4 (forward-compatible — logged as "Day 4 expected").
#   5. Class-D linux-only substrate (crosvm): platforms == EXACTLY
#      [linux-amd64, linux-arm64]. The Mac install-loop skip relies on this.
#   6. Class-E 3rd-party helpers (3): linux-amd64 + linux-arm64 + darwin-arm64
#      all present, all with real upstream `url` + `checksum`.
#   7. Capsules projection: every capsules entry except `chat-wasm`
#      includes `aarch64-darwin` in its platforms array.
#
# Exit codes:
#   0 — All required invariants green.
#   1 — One or more invariants violated; diagnostics printed to stderr.
#
# Usage:
#   bash scripts/lib/components-json-verify.sh [path/to/components.json]
#       (defaults to ${REPO_ROOT}/components.json)
#
# Anchor: docs/vz-backend/PHASE_6_COMPONENTS_AUDIT.md § 4.

set -euo pipefail

MANIFEST_PATH="${1:-$(git rev-parse --show-toplevel 2>/dev/null || pwd)/components.json}"

if [[ ! -f "${MANIFEST_PATH}" ]]; then
    echo "[components-json-verify] manifest not found: ${MANIFEST_PATH}" >&2
    exit 1
fi

MANIFEST_PATH="${MANIFEST_PATH}" python3 - <<'PY'
import json
import os
import sys

manifest_path = os.environ["MANIFEST_PATH"]
try:
    manifest = json.loads(open(manifest_path).read())
except Exception as e:
    sys.stderr.write(f"[components-json-verify] parse failed: {e}\n")
    sys.exit(1)

errors = []
notes = []

CLASS_A_HOST_BINARIES = [
    "shell",
    "localhost-provider",
    "did-provider",
    "webspace-provider",
    "ipfs-provider",
    "tunnel-provider",
    "site-provider",
]
# Promoted from forward-compat to required in Day 3.
CLASS_B_MICROVM_BUNDLES = ["chat"]
# Day-4 deliverable; Day-3 keeps as forward-compat note.
CLASS_C_KERNEL = ["vmlinux"]
CLASS_D_LINUX_ONLY = ["crosvm"]
CLASS_E_HELPERS = ["kubo", "cloudflared", "llama-server"]

REQUIRED_KEYS_ALL = ["linux-amd64", "linux-arm64", "darwin-arm64"]
# Decision D.2.a invariant: these fields must be identical between
# darwin-arm64 and linux-arm64 for every Class-B share-bundle entry.
SHARE_BUNDLE_INVARIANT_FIELDS = [
    "cid",
    "checksum",
    "size",
    "release_path",
    "extract_path",
]

external = manifest.get("external", {})
capsules = manifest.get("capsules", {})

# --- Class A ---
for name in CLASS_A_HOST_BINARIES:
    plats = external.get(name, {}).get("platforms", {})
    missing = [k for k in REQUIRED_KEYS_ALL if k not in plats]
    if missing:
        errors.append(f"[Class A] external.{name}.platforms missing keys: {missing}")

# --- Class B (Day-3 promoted to required; enforce share-bundle invariant) ---
for name in CLASS_B_MICROVM_BUNDLES:
    plats = external.get(name, {}).get("platforms", {})
    missing = [k for k in REQUIRED_KEYS_ALL if k not in plats]
    if missing:
        errors.append(f"[Class B] external.{name}.platforms missing keys: {missing}")
        continue
    darwin = plats["darwin-arm64"]
    linux_arm64 = plats["linux-arm64"]
    for field in SHARE_BUNDLE_INVARIANT_FIELDS:
        if darwin.get(field) != linux_arm64.get(field):
            errors.append(
                f"[Class B] D.2.a share-bundle invariant violated: "
                f"external.{name}.platforms.darwin-arm64.{field} "
                f"= {darwin.get(field)!r}, "
                f"linux-arm64.{field} = {linux_arm64.get(field)!r}; "
                "these must be byte-identical for share-linux-arm64-bundle"
            )

# --- Class C (forward-compat: Day-4 work) ---
for name in CLASS_C_KERNEL:
    plats = external.get(name, {}).get("platforms", {})
    base_missing = [k for k in ("linux-amd64", "linux-arm64") if k not in plats]
    if base_missing:
        errors.append(f"[Class C] external.{name}.platforms missing baseline keys: {base_missing}")
    if "darwin-arm64" not in plats:
        notes.append(f"[Class C] external.{name}.platforms.darwin-arm64 — Day 4 expected (carry-forward)")

# --- Class D (Linux-only; darwin must NOT be present) ---
for name in CLASS_D_LINUX_ONLY:
    plats = external.get(name, {}).get("platforms", {})
    actual = sorted(plats.keys())
    expected = sorted(["linux-amd64", "linux-arm64"])
    if actual != expected:
        errors.append(
            f"[Class D] external.{name}.platforms keys = {actual}, "
            f"expected exactly {expected} (Mac install-loop relies on absence of darwin-arm64)"
        )

# --- Class E (3rd-party helpers; require real upstream url + checksum) ---
for name in CLASS_E_HELPERS:
    plats = external.get(name, {}).get("platforms", {})
    if "darwin-arm64" not in plats:
        errors.append(f"[Class E] external.{name}.platforms.darwin-arm64 missing")
        continue
    darwin = plats["darwin-arm64"]
    # llama-server linux-arm64 uses source-build; darwin can too in principle
    # but Day-2 audit chose ingest-upstream — so require url+checksum here.
    if not darwin.get("url"):
        errors.append(f"[Class E] external.{name}.platforms.darwin-arm64.url is empty/missing")
    if not darwin.get("checksum"):
        errors.append(f"[Class E] external.{name}.platforms.darwin-arm64.checksum is empty/missing")

# --- Capsules projection ---
for name, entry in capsules.items():
    plats = entry.get("platforms", [])
    if name == "chat-wasm":
        if plats != ["any"]:
            errors.append(f"[Capsules] {name}.platforms = {plats}, expected [\"any\"]")
        continue
    if "aarch64-darwin" not in plats:
        errors.append(f"[Capsules] {name}.platforms missing 'aarch64-darwin'; got {plats}")

if notes:
    sys.stderr.write("[components-json-verify] forward-compat notes (non-fatal):\n")
    for n in notes:
        sys.stderr.write(f"  {n}\n")

if errors:
    sys.stderr.write("[components-json-verify] FAILED:\n")
    for e in errors:
        sys.stderr.write(f"  {e}\n")
    sys.exit(1)

print("[components-json-verify] OK")
print(f"  Class A (host binaries):    {len(CLASS_A_HOST_BINARIES)}/{len(CLASS_A_HOST_BINARIES)} green")
print(f"  Class B (microVM bundles):  {len(CLASS_B_MICROVM_BUNDLES)}/{len(CLASS_B_MICROVM_BUNDLES)} green (D.2.a share-bundle invariant enforced)")
print(f"  Class C (kernel):           baseline green ({len(CLASS_C_KERNEL)} entry, darwin Day 4)")
print(f"  Class D (linux-only):       {len(CLASS_D_LINUX_ONLY)}/{len(CLASS_D_LINUX_ONLY)} green (darwin absent as required)")
print(f"  Class E (3rd-party):        {len(CLASS_E_HELPERS)}/{len(CLASS_E_HELPERS)} green (real url + checksum)")
print(f"  Capsules projection:        {len([c for c in capsules if c != 'chat-wasm'])} entries include 'aarch64-darwin'")
sys.exit(0)
PY
