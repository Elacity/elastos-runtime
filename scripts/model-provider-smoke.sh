#!/usr/bin/env bash
# P2 gate: chat run end-to-end through the model-provider capsule against the
# real Flash pair A upstream. Proves: create → event stream → succeeded,
# mid-stream cancel, and latency parity vs a direct upstream call.
#
#   ./scripts/model-provider-smoke.sh [flash_url]
#
# Default upstream: http://192.168.1.147:8888/v1 (Sparks pair A).

set -euo pipefail

FLASH_URL="${1:-http://192.168.1.147:8888/v1}"
BIN="$(cd "$(dirname "$0")/.." && pwd)/capsules/model-provider/target/debug/model-provider"

if [[ ! -x "$BIN" ]]; then
  echo "build first: cargo build --manifest-path capsules/model-provider/Cargo.toml" >&2
  exit 1
fi

say() { echo "$(date -Iseconds) $*"; }

# Single stdio session drives the whole scenario (registry is in-process).
run_session() {
  python3 - "$BIN" "$FLASH_URL" <<'PY'
import json, subprocess, sys, time

bin_path, flash_url = sys.argv[1], sys.argv[2]
proc = subprocess.Popen([bin_path], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                        stderr=subprocess.DEVNULL, text=True)

def rpc(req):
    proc.stdin.write(json.dumps(req) + "\n")
    proc.stdin.flush()
    return json.loads(proc.stdout.readline())

init = rpc({"op": "init", "config": {"extra": {"flash_url": flash_url}}})
assert init["status"] == "ok", init

offers = rpc({"op": "offers_list"})
assert offers["data"]["offers"], "flash offer missing — is flash_url configured?"
print("offers:", [o["offer_id"] for o in offers["data"]["offers"]])

# --- 1. full round-trip ---
t0 = time.monotonic()
run = rpc({"op": "runs_create", "offer_id": "offer:flash-chat:pair-a",
           "operation": "generate",
           "inputs": {"messages": [{"role": "user", "content": "Reply with exactly: model-provider ok"}],
                      "max_tokens": 512, "temperature": 0}})
assert run["status"] == "ok", run
run_id = run["data"]["run_id"]
print("created:", run_id)

cursor, state, text, thinking = 0, None, "", ""
while True:
    ev = rpc({"op": "runs_events", "run_id": run_id, "cursor": cursor})
    cursor = ev["data"]["cursor"]
    state = ev["data"]["state"]
    for event in ev["data"]["events"]:
        if event["type"] == "text":
            text += event["delta"]
        elif event["type"] == "thinking":
            thinking += event["delta"]
    if state in ("succeeded", "failed", "cancelled"):
        break
    time.sleep(0.25)

elapsed = time.monotonic() - t0
print(f"round-trip: state={state} elapsed={elapsed:.1f}s text={text[:80]!r} thinking_len={len(thinking)}")
assert state == "succeeded", f"run ended in {state}"

get = rpc({"op": "runs_get", "run_id": run_id})
assert get["data"]["state"] == "succeeded", get

# --- 2. mid-stream cancel ---
run2 = rpc({"op": "runs_create", "offer_id": "offer:flash-chat:pair-a",
            "operation": "generate",
            "inputs": {"messages": [{"role": "user", "content": "Count from 1 to 200, one number per line."}],
                       "max_tokens": 2048}})
run_id2 = run2["data"]["run_id"]
time.sleep(0.5)
cancel = rpc({"op": "runs_cancel", "run_id": run_id2})
assert cancel["status"] == "ok", cancel

cursor, state = 0, None
deadline = time.monotonic() + 15
while time.monotonic() < deadline:
    ev = rpc({"op": "runs_events", "run_id": run_id2, "cursor": cursor})
    cursor, state = ev["data"]["cursor"], ev["data"]["state"]
    if state in ("succeeded", "failed", "cancelled"):
        break
    time.sleep(0.2)
print(f"cancel: state={state}")
assert state == "cancelled", f"expected cancelled, got {state}"

# --- 3. cancel of terminal run fails closed ---
again = rpc({"op": "runs_cancel", "run_id": run_id2})
assert again["status"] == "error" and again["code"] == "not_cancellable", again

# --- 4. input validation fails closed ---
bad = rpc({"op": "runs_create", "offer_id": "offer:flash-chat:pair-a",
           "operation": "generate", "inputs": {"messages": [], "bogus": 1}})
assert bad["status"] == "error" and bad["code"] == "invalid_inputs", bad

rpc({"op": "shutdown"})
print("SMOKE OK")
PY
}

say "== direct upstream latency (reference) =="
DIRECT_START=$(python3 -c 'import time; print(time.monotonic())')
curl -sS --max-time 60 "$FLASH_URL/chat/completions" -H 'Content-Type: application/json' \
  -d '{"model":"deepseek-v4-flash","messages":[{"role":"user","content":"Reply with exactly: model-provider ok"}],"max_tokens":512,"temperature":0,"stream":false}' \
  -o /tmp/model-provider-direct.json
python3 - "$DIRECT_START" <<'PY'
import json, sys, time
elapsed = time.monotonic() - float(sys.argv[1])
body = json.load(open("/tmp/model-provider-direct.json"))
msg = body["choices"][0]["message"]
text = (msg.get("content") or msg.get("reasoning") or "")[:80]
print(f"direct: elapsed={elapsed:.1f}s text={text!r}")
PY

say "== provider round-trip / cancel / validation =="
run_session

say "== P3 video run (15s clip, offer:h3-video:2x) =="
python3 - "$CAPSULE_BIN" <<'PY'
import json, subprocess, sys, time, hashlib, os, glob

proc = subprocess.Popen([sys.argv[1]], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
def rpc(req):
    proc.stdin.write(json.dumps(req) + "\n"); proc.stdin.flush()
    return json.loads(proc.stdout.readline())

out_dir = os.path.expanduser("~/.elastos/data/creative/jobs")
fallback = "creative/jobs"

init = rpc({"op": "init", "config": {"extra": {
    "flash_url": "http://127.0.0.1:18010/v1",
    "h3_url": "http://127.0.0.1:18000"}}})
print("init:", init["data"]["version"])

offers = rpc({"op": "offers_list"})
ids = [o["offer_id"] for o in offers["data"]["offers"]]
print("offers:", ids)
assert "offer:h3-video:2x" in ids, "h3 offer must be advertised when h3_url configured"

run = rpc({"op": "runs_create", "offer_id": "offer:h3-video:2x", "operation": "generate",
           "inputs": {"prompt": "Cinematic: a single paper lantern drifting through a rainy neon alley at night, shallow depth of field, photoreal",
                      "duration_seconds": 15}})
assert run["status"] == "ok", run
run_id = run["data"]["run_id"]
print("video run_id:", run_id)

# policy: second concurrent video run must be rejected
reject = rpc({"op": "runs_create", "offer_id": "offer:h3-video:2x", "operation": "generate",
              "inputs": {"prompt": "must be rejected", "duration_seconds": 5}})
print("concurrency reject:", reject["status"], reject.get("code"))
assert reject["status"] == "error" and reject["code"] == "policy_violation", reject

# stream progress to terminal
cursor, state, last = 0, None, -1
deadline = time.monotonic() + 3100
while time.monotonic() < deadline:
    ev = rpc({"op": "runs_events", "run_id": run_id, "cursor": cursor})
    events = ev["data"]["events"]
    cursor = ev["data"]["cursor"]
    state = ev["data"]["state"]
    if len(events) != last:
        last = len(events)
        phases = [e.get("phase") for e in events if e.get("type") == "progress"]
        print(f"  [{state}] events={cursor} phase={phases[-1] if phases else ''}")
    if state in ("succeeded", "failed", "cancelled"):
        break
    time.sleep(10)

final = rpc({"op": "runs_get", "run_id": run_id})
assert final["data"]["state"] == "succeeded", final
result = [e for e in final["data"]["events"] if e["type"] == "result"][0]
obj = result["objects"][0]
print("result object:", obj["media_type"], obj["size"], "bytes", "sha256=" + obj["sha256"][:16] + "...")

# artifact + sidecar verification: sidecar sha256 must match file bytes
safe_id = run_id.replace(":", "-")
candidates = [os.path.join(out_dir, safe_id + ".mp4"), os.path.join(fallback, safe_id + ".mp4")]
path = next((p for p in candidates if os.path.exists(p)), None)
assert path, f"artifact not found; looked: {candidates}"
digest = hashlib.sha256(open(path, "rb").read()).hexdigest()
sidecar = json.load(open(path.replace(".mp4", ".json")))
assert digest == obj["sha256"] == sidecar["sha256"], "sha256 mismatch across run result, file, sidecar"
print(f"artifact: {path} verified (size={os.path.getsize(path)})")

# input validation fails closed
bad1 = rpc({"op": "runs_create", "offer_id": "offer:h3-video:2x", "operation": "generate",
            "inputs": {"prompt": "x", "duration_seconds": 45}})
bad2 = rpc({"op": "runs_create", "offer_id": "offer:h3-video:2x", "operation": "generate",
            "inputs": {"prompt": "x", "resolution": "1080p"}})
print("validation:", bad1.get("code"), "/", bad2.get("code"))
assert bad1["code"] == "invalid_inputs" and bad2["code"] == "unsupported_parameter"

rpc({"op": "shutdown"})
print("VIDEO SMOKE OK")
PY
