#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

echo "[people-conversations-local-smoke] home entropy guard"
node "$ROOT/scripts/home-entropy-check.mjs"

echo "[people-conversations-local-smoke] Chat Room gateway creates ElastOS peer invite objects"
cargo test --manifest-path "$ROOT/elastos/Cargo.toml" -p elastos-server \
    test_chat_room_join_link_create_returns_elastos_join_object --lib -- --test-threads=1

echo "[people-conversations-local-smoke] Home preserves peer invite launch query"
cargo test --manifest-path "$ROOT/elastos/Cargo.toml" -p elastos-server \
    test_home_launch_validates_shell_targets --lib -- --test-threads=1

echo "[people-conversations-local-smoke] People stores profile cards"
cargo test --manifest-path "$ROOT/elastos/Cargo.toml" -p elastos-server \
    test_system_handle_derives_from_passkey_principal --lib -- --test-threads=1

echo "[people-conversations-local-smoke] People projects accepted conversation contacts"
cargo test --manifest-path "$ROOT/elastos/Cargo.toml" -p elastos-server \
    test_home_summary_reports_people_contacts_from_accepted_conversation_members --lib -- --test-threads=1

echo "[people-conversations-local-smoke] Chat UI decodes Home launch invite query"
cargo test --manifest-path "$ROOT/capsules/chat-room-ui/Cargo.toml" --lib \
    decodes_query_value_for_invite_urls

echo "[people-conversations-local-smoke] PASS"
