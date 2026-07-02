#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

echo "[carrier-only] alignment gate"
bash scripts/check-wci-alignment.sh

echo "[carrier-only] runtime Carrier tests"
(cd elastos && cargo test -p elastos-runtime carrier -- --nocapture)

echo "[carrier-only] server Carrier provider-plane tests"
(cd elastos && cargo test -p elastos-server carrier_provider_invoke -- --nocapture)

echo "[carrier-only] server remote Carrier exit bridge tests, including Browser byte relay"
(cd elastos && cargo test -p elastos-server remote_carrier -- --nocapture)

echo "[carrier-only] exit-provider remote Carrier policy tests"
cargo test --manifest-path capsules/exit-provider/Cargo.toml remote_carrier -- --nocapture

echo "[carrier-only] remote Carrier operator evidence contract"
bash scripts/remote-carrier-exit-operator-report-smoke.sh

echo "[carrier-only] remote Carrier installed-config readiness contract"
bash scripts/remote-carrier-exit-readiness-smoke.sh

echo "[carrier-only] remote Carrier installed-artifact readiness contract"
bash scripts/remote-carrier-exit-artifact-readiness-smoke.sh

echo "[carrier-only] remote Carrier public-live update plan contract"
bash scripts/remote-carrier-exit-public-live-plan-smoke.sh

echo "[carrier-only] remote Carrier source-config setup contract"
bash scripts/remote-carrier-exit-source-config-smoke.sh

cat <<'JSON'
{
  "schema": "elastos.carrier-only-authority-check/v1",
  "ok": true,
  "covered_contracts": [
    "wci_alignment_gate",
    "runtime_carrier",
    "server_carrier_provider_plane",
    "remote_carrier_exit_bridge",
    "browser_carrier_byte_relay",
    "exit_provider_remote_carrier_policy",
    "remote_carrier_operator_evidence",
    "remote_carrier_installed_config_readiness",
    "remote_carrier_installed_artifact_readiness",
    "remote_carrier_public_live_update_plan",
    "remote_carrier_source_config_setup"
  ]
}
JSON
