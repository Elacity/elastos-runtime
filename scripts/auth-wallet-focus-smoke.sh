#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

run() {
  printf '\n==> %s\n' "$*"
  "$@"
}

run cargo test --manifest-path elastos/Cargo.toml -p elastos-server passkey -- --nocapture
run cargo test --manifest-path elastos/Cargo.toml -p elastos-identity extension_payloads -- --nocapture
run cargo test --manifest-path elastos/Cargo.toml -p elastos-server recovery -- --nocapture
run cargo test --manifest-path elastos/Cargo.toml -p elastos-auth principal_root_protection -- --nocapture
run cargo test --manifest-path elastos/Cargo.toml -p elastos-server carrier_bridge -- --nocapture
run cargo test --manifest-path elastos/Cargo.toml -p elastos-server principal_launch -- --nocapture
run cargo test --manifest-path elastos/Cargo.toml -p elastos-server supervisor_launch -- --nocapture
run cargo test --manifest-path elastos/Cargo.toml -p elastos-server test_home_launch_starts_chat_room_capsule_and_reports_runtime_activity -- --nocapture
run cargo test --manifest-path elastos/Cargo.toml -p elastos-server wallet_connector -- --nocapture
run cargo test --manifest-path elastos/Cargo.toml -p elastos-server walletconnect -- --nocapture
run cargo test --manifest-path elastos/Cargo.toml -p elastos-server metamask_connector -- --nocapture
run cargo test --manifest-path elastos/Cargo.toml -p elastos-server unisat -- --nocapture
run cargo test --manifest-path elastos/Cargo.toml -p elastos-server wallet_token_cannot_link_bip322_account -- --nocapture
run cargo test --manifest-path elastos/Cargo.toml -p elastos-server system_can_create_managed_wallet_account -- --nocapture
run cargo test --manifest-path elastos/Cargo.toml -p elastos-server system_can_select_default_wallet -- --nocapture
run cargo test --manifest-path elastos/Cargo.toml -p elastos-server system_approves_managed_wallet_request -- --nocapture
run cargo test --manifest-path elastos/Cargo.toml -p elastos-server wallet_approval -- --nocapture
run cargo test --manifest-path capsules/wallet-provider/Cargo.toml managed -- --nocapture
run cargo test --manifest-path capsules/wallet-provider/Cargo.toml bip322 -- --nocapture
run cargo test --manifest-path capsules/chain-provider/Cargo.toml prepares_typed_evm_transaction_intent -- --nocapture
run cargo test --manifest-path capsules/chain-provider/Cargo.toml broadcasts_typed_evm_signed_transaction -- --nocapture
run cargo test --manifest-path capsules/chain-provider/Cargo.toml has_access_by_content_id -- --nocapture
run cargo test --manifest-path capsules/chain-provider/Cargo.toml erc1271 -- --nocapture
run cargo test --manifest-path capsules/chain-provider/Cargo.toml sync_health -- --nocapture
run cargo test --manifest-path capsules/chain-provider/Cargo.toml node_lifecycle -- --nocapture
run cargo test --manifest-path elastos/Cargo.toml -p elastos-server gateway_blocks_chain_proof_prepare_and_broadcast_routes -- --nocapture
run bash -n scripts/vendor-walletconnect-adapter.sh
run scripts/walletconnect-connector-config-smoke.sh
run node scripts/home-fresh-passkey-authority-smoke.mjs
run node scripts/home-entropy-check.mjs
run scripts/check-wci-alignment.sh
run scripts/recovery-kit-live-smoke.sh
