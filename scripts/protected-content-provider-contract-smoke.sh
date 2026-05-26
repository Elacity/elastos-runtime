#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP_ROOT="$(mktemp -d)"
trap 'rm -rf "$TMP_ROOT"' EXIT

export ELASTOS_PROVIDER_SMOKE_TARGET_DIR="$TMP_ROOT/target"

python3 - "$ROOT" <<'PY'
import copy
import json
import os
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(sys.argv[1])
TARGET_DIR = pathlib.Path(os.environ["ELASTOS_PROVIDER_SMOKE_TARGET_DIR"])
TIMEOUT_SECONDS = 120


def assert_true(condition, message):
    if not condition:
        raise AssertionError(message)


def assert_eq(left, right, message):
    if left != right:
        raise AssertionError(f"{message}: expected {right!r}, got {left!r}")


def provider_roundtrip(name, requests):
    manifest = ROOT / "capsules" / name / "Cargo.toml"
    payload = "\n".join(json.dumps(request, separators=(",", ":")) for request in requests) + "\n"
    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = str(TARGET_DIR)
    result = subprocess.run(
        ["cargo", "run", "--quiet", "--manifest-path", str(manifest)],
        input=payload,
        text=True,
        capture_output=True,
        env=env,
        timeout=TIMEOUT_SECONDS,
        check=False,
    )
    if result.returncode != 0:
        sys.stderr.write(result.stderr)
        raise SystemExit(
            f"[protected-content-provider-contract] {name} exited with {result.returncode}"
        )
    lines = [line for line in result.stdout.splitlines() if line.strip()]
    assert_eq(len(lines), len(requests), f"{name} response count")
    return [json.loads(line) for line in lines]


def ok_data(response, name):
    assert_eq(response.get("status"), "ok", f"{name} status")
    assert_true("data" in response, f"{name} missing data")
    return response["data"]


def error_code(response, code, name):
    assert_eq(response.get("status"), "error", f"{name} status")
    assert_eq(response.get("code"), code, f"{name} error code")


KEY_ENVELOPE = {
    "scheme": "elastos-pq-hybrid-threshold-v0",
    "kid": "kid:test",
    "wrapped_cek": "wrapped",
    "policy_hash": "sha256:test",
    "algorithms": {
        "cipher": "aes-256-gcm",
        "signature": ["ed25519", "ml-dsa-65"],
        "kem": ["x25519", "ml-kem-768"],
        "share_scheme": "shamir-t-of-n",
    },
}

SEALED_OBJECT = {
    "schema": "elastos.sealed.object/v1",
    "payload_cid": "bafybeigpayload",
    "rights_policy_cid": "bafybeigpolicy",
    "availability_receipt_cid": "bafybeigreceipt",
    "key_envelope": KEY_ENVELOPE,
    "viewer": {"required_interface": "elastos.viewer/document@1"},
}

DRM_OPEN = {
    "object": SEALED_OBJECT,
    "principal_id": "person:local:test",
    "session_id": "session:test",
    "action": "view",
    "reason": "open protected document",
}

RIGHTS_ACCESS = {
    "principal_id": "person:local:test",
    "session_id": "session:test",
    "content_id": "bafybeigprotectedcontent",
    "right": "view",
    "reason": "open protected document",
    "policy_ref": "bafybeigpolicy",
}

KEY_RELEASE = {
    "schema": "elastos.key_release.request/v1",
    "request_id": "key-release:test",
    "principal_id": "person:local:test",
    "session_id": "session:test",
    "object_cid": "bafybeigprotectedcontent",
    "action": "view",
    "key_envelope": KEY_ENVELOPE,
    "reason": "open protected document",
    "expires_at": 1_900_000_000,
}

DECRYPT_SESSION = {
    "schema": "elastos.decrypt.session.request/v1",
    "request_id": "decrypt:test",
    "principal_id": "person:local:test",
    "session_id": "session:test",
    "object_cid": "bafybeigprotectedcontent",
    "action": "view",
    "viewer_interface": "elastos.viewer/document@1",
    "release_receipt_id": "key-release:test",
    "output_kind": "rendered",
    "reason": "open protected document",
    "expires_at": 1_900_000_000,
}


def check_drm_provider():
    invalid = copy.deepcopy(DRM_OPEN)
    invalid["action"] = "raw_key"
    responses = provider_roundtrip(
        "drm-provider",
        [
            {"op": "status"},
            {"op": "open", "request": DRM_OPEN},
            {"op": "open", "request": invalid},
            {"op": "shutdown"},
        ],
    )
    data = ok_data(responses[0], "drm status")
    blocked = set(data["blocked_authority"])
    assert_true(
        {"raw_cek", "wallet_rpc", "chain_rpc", "kubo_api", "elacity_sdk"} <= blocked,
        "drm blocked authority",
    )
    steps = [step["step"] for step in data["required_sequence"]]
    assert_eq(
        steps,
        [
            "content_status",
            "content_fetch",
            "rights_check",
            "key_release",
            "decrypt_session",
            "render",
            "release_receipt",
            "audit",
        ],
        "drm required sequence",
    )
    error_code(responses[1], "not_configured", "drm open")
    detail_steps = [step["step"] for step in responses[1]["details"]["required_sequence"]]
    assert_eq(detail_steps, steps, "drm open detail sequence")
    error_code(responses[2], "invalid_request", "drm invalid action")
    assert_eq(responses[3].get("status"), "ok", "drm shutdown")


def check_rights_provider():
    invalid = copy.deepcopy(RIGHTS_ACCESS)
    invalid["content_id"] = "../secret"
    responses = provider_roundtrip(
        "rights-provider",
        [
            {"op": "status"},
            {"op": "has_access_by_content_id", "request": RIGHTS_ACCESS},
            {"op": "has_access_by_content_id", "request": invalid},
            {"op": "shutdown"},
        ],
    )
    data = ok_data(responses[0], "rights status")
    blocked = set(data["blocked_authority"])
    assert_true(
        {"contract_sdk", "chain_rpc", "wallet_rpc", "raw_cek"} <= blocked,
        "rights blocked authority",
    )
    error_code(responses[1], "not_configured", "rights access")
    error_code(responses[2], "invalid_request", "rights invalid content_id")
    assert_eq(responses[3].get("status"), "ok", "rights shutdown")


def check_key_provider():
    invalid = copy.deepcopy(KEY_RELEASE)
    invalid["key_envelope"]["algorithms"]["cipher"] = "aes-128-gcm"
    responses = provider_roundtrip(
        "key-provider",
        [
            {"op": "status"},
            {"op": "release", "request": KEY_RELEASE},
            {"op": "release", "request": invalid},
            {"op": "shutdown"},
        ],
    )
    data = ok_data(responses[0], "key status")
    blocked = set(data["blocked_authority"])
    assert_true(
        {"raw_cek", "kms_node_credentials", "chain_rpc", "wallet_rpc"} <= blocked,
        "key blocked authority",
    )
    error_code(responses[1], "not_configured", "key release")
    error_code(responses[2], "invalid_request", "key weak cipher")
    assert_eq(responses[3].get("status"), "ok", "key shutdown")


def check_decrypt_provider():
    invalid = copy.deepcopy(DECRYPT_SESSION)
    invalid["output_kind"] = "raw_plaintext"
    responses = provider_roundtrip(
        "decrypt-provider",
        [
            {"op": "status"},
            {"op": "open_session", "request": DECRYPT_SESSION},
            {"op": "open_session", "request": invalid},
            {"op": "shutdown"},
        ],
    )
    data = ok_data(responses[0], "decrypt status")
    blocked = set(data["blocked_authority"])
    assert_true(
        {"raw_cek", "raw_plaintext", "filesystem", "chain_rpc", "wallet_rpc"} <= blocked,
        "decrypt blocked authority",
    )
    error_code(responses[1], "not_configured", "decrypt session")
    error_code(responses[2], "invalid_request", "decrypt raw plaintext")
    assert_eq(responses[3].get("status"), "ok", "decrypt shutdown")


check_drm_provider()
check_rights_provider()
check_key_provider()
check_decrypt_provider()

print("[protected-content-provider-contract] PASS")
PY
