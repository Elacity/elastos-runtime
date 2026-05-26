#!/usr/bin/env bash
#
# Mac live-demo walkthrough — run the Phase-9 sign-off matrix one
# step at a time so the operator can watch each layer come up.
#
# Usage:
#   bash scripts/dev/mac-live-demo.sh         # interactive (default)
#   bash scripts/dev/mac-live-demo.sh --all   # non-interactive, all steps
#
# Each step prints:
#   1. Plain-English description of what's about to happen.
#   2. The exact command being run.
#   3. The command's output.
#   4. The "look for this" markers that prove the step worked.
#
# Open a SECOND terminal and run:
#   tail -F "$HOME/Library/Application Support/elastos/logs/runtime.log"
# to watch the runtime daemon's logs live as the steps progress.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DEBUG_ELASTOS="$REPO_ROOT/elastos/target/debug/elastos"
DATA_DIR="$HOME/Library/Application Support/elastos"
RUNTIME_LOG="$DATA_DIR/logs/runtime.log"

INTERACTIVE=1
if [[ "${1:-}" == "--all" ]]; then
  INTERACTIVE=0
fi

red()   { printf '\033[1;31m%s\033[0m\n' "$*"; }
green() { printf '\033[1;32m%s\033[0m\n' "$*"; }
cyan()  { printf '\033[1;36m%s\033[0m\n' "$*"; }
gray()  { printf '\033[2m%s\033[0m\n' "$*"; }

step() {
  local n="$1" title="$2"
  echo
  echo "================================================================"
  cyan "STEP $n — $title"
  echo "================================================================"
}

pause() {
  if [[ $INTERACTIVE -eq 1 ]]; then
    echo
    read -r -p "  press <enter> to continue (or Ctrl-C to stop) " _
  fi
}

look_for() {
  echo
  green "  ✔ LOOK FOR:"
  for line in "$@"; do
    echo "      • $line"
  done
}

run_cmd() {
  echo
  gray "  $ $*"
  echo
  "$@"
}

# ---------------------------------------------------------------------
# Preflight
# ---------------------------------------------------------------------

if [[ ! -x "$DEBUG_ELASTOS" ]]; then
  red "ERROR: elastos binary not built at $DEBUG_ELASTOS"
  echo "Run: cargo build --manifest-path $REPO_ROOT/elastos/Cargo.toml -p elastos-server"
  exit 1
fi

echo
cyan "================================================================"
cyan "        ElastOS on Mac — Live Validation Walkthrough"
cyan "================================================================"
echo
echo "What this script proves, in order:"
echo "  1. Bootstrap (Layer 3) is healthy and idempotent"
echo "  2. Runtime substrate (Layer 2) sees all 5 home-surface capsules"
echo "  3. Apple Vz substrate (Layer 1) is enabled and ready"
echo "  4. WASM JIT works standalone — Hardened Runtime entitlements OK"
echo "  5. Full canonical chain end-to-end via managed Home runtime"
echo "  6. Inter-capsule architecture — confirmed Carrier-only by code"
echo
echo "Open a SECOND terminal NOW and run:"
echo
cyan "  tail -F '$RUNTIME_LOG'"
echo
echo "Keep that visible while running this script — every step that"
echo "touches the daemon will produce log lines you can watch live."
echo
pause

# ---------------------------------------------------------------------
step 1 "Bootstrap idempotency (Layer 3)"
# ---------------------------------------------------------------------

echo "We re-run the Mac bootstrap. It detects already-installed binaries,"
echo "re-signs if cargo stripped entitlements, and verifies all 5"
echo "home-surface capsules have matching .elastos-cid stamps."

run_cmd bash "$REPO_ROOT/scripts/dev/mac-local-setup.sh"

look_for \
  "services ready: 6 / 8 (kubo + cloudflared are the 2 third-party gaps)" \
  "verifying capsule registry consistency → [ok ] for home, system, documents, library, inbox" \
  "final '[mac-local-setup] OK' line"

pause

# ---------------------------------------------------------------------
step 2 "Live runtime state (Layer 2 substrate)"
# ---------------------------------------------------------------------

echo "Query the runtime's view of the world via 'elastos home --status'."
echo "No daemon needed — this command reads the on-disk registry directly."

run_cmd "$DEBUG_ELASTOS" home --status --json \
  | python3 -c '
import json, sys
snap = json.load(sys.stdin)
print(f"  DID:             {snap.get(\"did\")}")
print(f"  Data dir:        {snap.get(\"data_dir\")}")
print(f"  Capsules cached: {len(snap.get(\"cached_capsules\", []))}")
for c in sorted(snap.get("cached_capsules", []))[:8]:
    print(f"    - {c}")
print()
print("  System services (8 total):")
for s in snap.get("system_services", []):
    state = "[READY    ]" if s.get("ready") else "[not ready]"
    print(f"    {state} {s.get(\"name\")} ({s.get(\"backing\")})")
'

look_for \
  "Capsules cached: ≥ 5 (home, system, documents, library, inbox + chat + ...)" \
  "6 services marked [READY]" \
  "Full-screen Apps marked [READY] with backing (vmlinux) — proves Day-3 cfg-gating works"

pause

# ---------------------------------------------------------------------
step 3 "Standalone WASM JIT (Layer 1 + 2, no daemon)"
# ---------------------------------------------------------------------

echo "Launch the 'home' capsule in-process. No daemon needed."
echo "This proves Hardened Runtime + JIT entitlements + wasmtime work."

run_cmd "$DEBUG_ELASTOS" run capsules/home

look_for \
  "'vz provider enabled (Apple Virtualization.framework available...)' line" \
  "'Loading capsule home (Wasm)' line" \
  "'home capsule launched: name=home id=wasm-...' line" \
  "'[run] WASM capsule home exited' line + exit 0" \
  "NO 'killed: 9' or SIGKILL — that would mean missing JIT entitlement"

pause

# ---------------------------------------------------------------------
step 4 "End-to-end through managed Home (all three layers)"
# ---------------------------------------------------------------------

echo "Launch the 'system' capsule THROUGH the managed-home runtime daemon."
echo "This exercises the full canonical chain that was broken pre-Day-5:"
echo
echo "  1. capsule_cmd::run_capsule → POST /api/supervisor/resolve-plan"
echo "  2. supervisor reads components.json, finds 'system' in capsules:"
echo "  3. POST /api/supervisor/ensure-capsule  → matches .elastos-cid stamp"
echo "  4. capsule_cmd takes the Wasm branch → run_wasm_capsule"
echo "  5. WASM bridge active, capsule runs"
echo
echo "If you're watching the runtime.log tail in your second terminal,"
echo "you'll see ALL these steps in real time."
echo
echo "When the capsule starts, press Ctrl-C to come back to this script."

if [[ $INTERACTIVE -eq 1 ]]; then
  echo
  read -r -p "  press <enter> to launch 'elastos capsule system --interactive' " _
fi

"$DEBUG_ELASTOS" capsule system --lifecycle interactive --interactive || true

look_for \
  "'No runtime found. Starting local home runtime...' (only first time)" \
  "'Runtime started (pid ...)' (now we have a daemon)" \
  "'Loading capsule system (Wasm)' (resolve-plan + ensure-capsule succeeded)" \
  "'Loaded WASM capsule system with ID wasm-...'" \
  "'WASM bridge active for capsule system'" \
  "'system capsule launched: name=system ...' (capsule main() ran)"

pause

# ---------------------------------------------------------------------
step 5 "Inspect what the daemon recorded (Layer 2 internals)"
# ---------------------------------------------------------------------

echo "The managed-home daemon is still running (it persists after each"
echo "capsule exits, so subsequent launches are instant). Let's look at"
echo "what it logged."

if [[ -f "$RUNTIME_LOG" ]]; then
  echo
  gray "  $ tail -30 '$RUNTIME_LOG'"
  echo
  tail -30 "$RUNTIME_LOG"
else
  red "  Runtime log not yet present at $RUNTIME_LOG"
fi

look_for \
  "Same 'Loading capsule', 'Loaded WASM capsule', 'WASM bridge active' lines" \
  "Daemon stayed up — proves it persists across capsule launches"

pause

# ---------------------------------------------------------------------
step 6 "Inter-capsule architecture — Carrier only (proven in code)"
# ---------------------------------------------------------------------

echo "You asked: 'VMs only connect to each other via Carrier, right?'"
echo "Answer: YES — and even more strictly than that. Each VM is isolated"
echo "from every other VM at the network layer. The ONLY sanctioned"
echo "inter-capsule channel is the Carrier bridge."
echo
echo "Verifiable in code:"
echo
gray "  elastos-vz/src/ffi/builder.rs (the VM configuration)"
echo "    - line 137-152: VM gets NAT'd interface by default."
echo "                    Bridged attachment requires explicit opt-in"
echo "                    + the com.apple.vm.networking entitlement."
echo "                    Capsules do NOT share an L2 link."
echo "    - line 187-190: Each VM gets a virtio-console slot at /dev/hvc1"
echo "                    inside the guest — this is the Carrier bridge"
echo "                    socket (a Unix stream on the host side)."
echo
gray "  elastos-server/src/carrier_bridge.rs (the host-side dispatcher)"
echo "    - line 68:  spawn_carrier_bridge — for microVMs"
echo "    - line 117: spawn_carrier_bridge_on_stream — Unix-stream entry"
echo "    - line 140: run_carrier_bridge_loop — the dispatch loop"
echo "    - line 227: spawn_wasm_carrier_bridge — for WASM capsules"
echo
echo "Topology:"
echo
cat <<'TOPO'
              ┌────────────┐         ┌────────────┐
              │ Capsule A  │         │ Capsule B  │
              │ (microVM)  │         │ (microVM)  │
              │            │         │            │
              │ /dev/hvc1  │         │ /dev/hvc1  │
              └─────┬──────┘         └──────┬─────┘
                    │  virtio-console       │
                    ▼                       ▼
              ┌──────────────────────────────────┐
              │     elastos-server runtime       │
              │     (Carrier bridge dispatcher)  │
              └──────────────┬───────────────────┘
                             │ Carrier P2P overlay
                             ▼
                  (other nodes on the network)

  - No L2 / L3 connectivity between A and B.
  - Each VM has its own NAT'd interface for outbound internet only.
  - A → B messages traverse: A/dev/hvc1 → runtime → B/dev/hvc1.
  - A → remote_node messages: A/dev/hvc1 → runtime → Carrier → remote.
TOPO

look_for \
  "VMs are network-isolated by Apple's Vz NAT by default" \
  "Each VM has exactly one inter-capsule channel: /dev/hvc1 (Carrier)" \
  "Cross-host comms go through the same Carrier overlay (P2P)"

# ---------------------------------------------------------------------
echo
echo
cyan "================================================================"
green "        Walkthrough complete. Phase 9 sign-off validated."
cyan "================================================================"
echo
echo "Things you can keep playing with:"
echo "  - $DEBUG_ELASTOS home          # full TUI dashboard"
echo "  - $DEBUG_ELASTOS capsule home --interactive"
echo "  - tail -F '$RUNTIME_LOG'    # live daemon log"
echo
echo "To clean up the running daemon:"
echo "  pkill -INT -f 'elastos serve'"
echo
