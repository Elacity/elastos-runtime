#!/usr/bin/env bash
set -euo pipefail
umask 077

# Isolated installed-consumer proof for a pinned small model.
#
# The harness installs the Runtime-verified local model engine into a
# disposable data root through the ordinary `elastos setup` path, verifies the
# caller-owned model and license bytes against their pinned identity, and then
# runs the opt-in real-engine proofs that the elastos-server test suite owns:
# native startup/reply/cancel/restart, and cold loopback Content admission
# followed by reply, restart and zero-download reuse. Every process, port and
# temporary path belongs to this run; the receipt binds the source tree, the
# built binaries, the installed engine receipt and the proof outcomes.
#
# This proves engine activation on the host it runs on. It is not Marketplace
# delivery evidence: the model bytes arrive from a caller-supplied file, the
# catalog is signed with an isolated test key, and no public holder is used.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
    cat <<'EOF'
Usage:
  scripts/pinned-model-consumer-proof.sh <proof-root> [--fetch] [--native-only]
  scripts/pinned-model-consumer-proof.sh <proof-root> --check-inputs
  scripts/pinned-model-consumer-proof.sh --self-test

Arguments:
  <proof-root>    New or existing directory owned by this proof. It receives
                  home/ (isolated HOME with the installed engine), kubo/,
                  inputs/ (with --fetch) and proof/ (logs and receipt.json).
  --fetch         Download the pinned model and license into <proof-root>/inputs
                  when ELASTOS_PROOF_MODEL_PATH / _LICENSE_PATH are unset.
  --native-only   Skip the cold Content proof (no Kubo or ipfs-provider).
  --check-inputs  Verify pinned inputs and print the plan; run nothing else.
  --self-test     Check the test-output verifier against captured cargo output
                  shapes (zero matches, unselected ignored test, wrong test,
                  one real pass); run nothing else.

Environment:
  ELASTOS_PROOF_MODEL          Pinned model id. Default and only value: smollm2
  ELASTOS_PROOF_MODEL_PATH     Read-only pinned GGUF file (owner-only write)
  ELASTOS_PROOF_LICENSE_PATH   Read-only license text with the pinned digest
  ELASTOS_PROOF_ELASTOS_BIN    Prebuilt `elastos` binary (default: cargo build)
  ELASTOS_PROOF_MODEL_PROVIDER Prebuilt model-provider (default: cargo build)
  ELASTOS_PROOF_IPFS_PROVIDER  Prebuilt ipfs-provider (default: cargo build)
  ELASTOS_PROOF_ACCOMMODATIONS Named temporary source accommodations recorded
                               in the receipt; required when the tree is dirty
  ELASTOS_CARGO_BIN            cargo executable. Default: cargo
EOF
}

die() {
    echo "[pinned-model-proof] $*" >&2
    exit 1
}

need_cmd() {
    command -v "$1" >/dev/null 2>&1 || die "missing required command: $1"
}

sha256_file() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

# Pinned model table. Bytes and digests are the same on every host; the
# engine executable identity is per platform and lives in the Rust proof.
pinned_field() {
    local model="$1" field="$2"
    case "${model}:${field}" in
        smollm2:file) echo "SmolLM2-135M-Instruct-Q8_0.gguf" ;;
        smollm2:url) echo "https://huggingface.co/unsloth/SmolLM2-135M-Instruct-GGUF/resolve/9e6855bc4be717fca1ef21360a1db4b29d5c559a/SmolLM2-135M-Instruct-Q8_0.gguf" ;;
        smollm2:bytes) echo "144811072" ;;
        smollm2:sha256) echo "c4a3dd037301b6ecea31d6da37f5cd793ead920dd5ddfe6d589294628d6ce66a" ;;
        smollm2:license_url) echo "https://www.apache.org/licenses/LICENSE-2.0.txt" ;;
        smollm2:license_bytes) echo "11358" ;;
        smollm2:license_sha256) echo "cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30" ;;
        smollm2:env_prefix) echo "SMOLLM2" ;;
        smollm2:native_test) echo "model_native_smollm2_exact_profile_lifecycle" ;;
        smollm2:cold_test) echo "model_preparation_real_smollm2_cold_reply_restart" ;;
        *) die "unknown pinned model field ${model}:${field}" ;;
    esac
}

detect_platform() {
    case "$(uname -s)-$(uname -m)" in
        Linux-x86_64) echo "linux-amd64" ;;
        Darwin-arm64) echo "darwin-arm64" ;;
        *) die "no Runtime-owned model host profile for $(uname -s)-$(uname -m)" ;;
    esac
}

verify_pinned_file() {
    local label="$1" path="$2" bytes="$3" sha="$4"
    [[ -f "$path" && ! -L "$path" ]] || die "$label is not a regular file: $path"
    local actual_bytes
    actual_bytes="$(wc -c <"$path" | tr -d ' ')"
    [[ "$actual_bytes" == "$bytes" ]] || die "$label size ${actual_bytes} differs from pinned ${bytes}"
    local actual_sha
    actual_sha="$(sha256_file "$path")"
    [[ "$actual_sha" == "$sha" ]] || die "$label sha256 ${actual_sha} differs from pinned ${sha}"
    echo "[pinned-model-proof] $label verified: ${bytes} bytes sha256 ${sha}"
}

fetch_pinned() {
    local url="$1" dest="$2"
    need_cmd curl
    if [[ ! -f "$dest" ]]; then
        echo "[pinned-model-proof] downloading $(basename "$dest")"
        curl -fsSL --retry 3 -o "${dest}.partial" "$url"
        mv -f -- "${dest}.partial" "$dest"
    fi
    chmod 0400 "$dest"
}

PROOF_MODULE="api::capsule_inventory::preparation::tests::process_proof"

# A cargo exit status of zero does not prove that the intended test ran: a
# filter that matches nothing runs zero tests and still exits zero, and an
# ignored test that is not selected prints "ignored" and exits zero. The log
# must show exactly the intended test executing and passing, once, and the
# summary must count exactly that one pass. Prints the refusal reason.
verify_proof_output() {
    local log="$1" test_id="$2"
    [[ -s "$log" ]] || { echo "empty test output"; return 1; }
    if grep -qF "test ${test_id} ... ignored" "$log"; then
        echo "the intended test was ignored, not executed"
        return 1
    fi
    if grep -qF "test ${test_id} ... FAILED" "$log"; then
        echo "the intended test failed"
        return 1
    fi
    local executed
    executed="$(grep -cxF "test ${test_id} ... ok" "$log" || true)"
    if [[ "$executed" != "1" ]]; then
        echo "expected exactly one passing execution of ${test_id}, found ${executed}"
        return 1
    fi
    if ! grep -qE '^test result: ok\. 1 passed; 0 failed; 0 ignored; 0 measured; [0-9]+ filtered out' "$log"; then
        echo "the summary does not count exactly one passing test"
        return 1
    fi
}

# Regression for the verifier on captured cargo output shapes: a zero-match
# filter and an unselected ignored test are refused, a wrong test is refused,
# and one real passing execution is accepted.
verifier_self_test() {
    local intended="${PROOF_MODULE}::model_native_smollm2_exact_profile_lifecycle"
    local scratch
    scratch="$(mktemp -d)"
    cat >"${scratch}/zero-match.log" <<'EOF'
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.31s
     Running unittests src/lib.rs (/build/deps/elastos_server-2795cde28a08a7f8)

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1903 filtered out; finished in 0.00s

EOF
    cat >"${scratch}/not-selected.log" <<EOF
     Running unittests src/lib.rs (/build/deps/elastos_server-2795cde28a08a7f8)

running 1 test
test ${intended} ... ignored, requires exact read-only SmolLM2 weights, verified engine and native provider

test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 1902 filtered out; finished in 0.00s

EOF
    cat >"${scratch}/wrong-test.log" <<EOF
     Running unittests src/lib.rs (/build/deps/elastos_server-2795cde28a08a7f8)

running 1 test
test ${PROOF_MODULE}::model_native_qwen_exact_profile_lifecycle ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1902 filtered out; finished in 4.17s

EOF
    cat >"${scratch}/one-pass.log" <<EOF
     Running unittests src/lib.rs (/build/deps/elastos_server-2795cde28a08a7f8)

running 1 test
smollm2-isolated-proof native proof journal: /tmp/.tmpyusMSR
smollm2-isolated-proof native Init elapsed_ms=3
pinned model terminal {"error_class":"","error_code":"","error_message":"","status":"completed"}
smollm2-isolated-proof completed reply chars=10 head="I'm ready."
smollm2-isolated-proof native active_delta_bytes=1674; exact restart replay passed
test ${intended} ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1902 filtered out; finished in 4.17s

EOF
    local failures=0 reason
    for refused in zero-match not-selected wrong-test; do
        if reason="$(verify_proof_output "${scratch}/${refused}.log" "$intended")"; then
            echo "[pinned-model-proof] self-test: ${refused} output was accepted" >&2
            failures=$((failures + 1))
        else
            echo "[pinned-model-proof] self-test: ${refused} refused (${reason})"
        fi
    done
    if reason="$(verify_proof_output "${scratch}/one-pass.log" "$intended")"; then
        echo "[pinned-model-proof] self-test: one real passing execution accepted"
    else
        echo "[pinned-model-proof] self-test: real passing output refused (${reason})" >&2
        failures=$((failures + 1))
    fi
    rm -rf -- "$scratch"
    [[ "$failures" -eq 0 ]] || die "verifier self-test failed (${failures})"
}

PROOF_ROOT=""
FETCH=0
NATIVE_ONLY=0
CHECK_INPUTS=0
for arg in "$@"; do
    case "$arg" in
        --help | -h) usage; exit 0 ;;
        --self-test) verifier_self_test; exit 0 ;;
        --fetch) FETCH=1 ;;
        --native-only) NATIVE_ONLY=1 ;;
        --check-inputs) CHECK_INPUTS=1 ;;
        --*) die "unknown option: $arg" ;;
        *)
            [[ -z "$PROOF_ROOT" ]] || die "only one proof root is accepted"
            PROOF_ROOT="$arg"
            ;;
    esac
done
[[ -n "$PROOF_ROOT" ]] || { usage >&2; exit 2; }

need_cmd python3
need_cmd tar
MODEL="${ELASTOS_PROOF_MODEL:-smollm2}"
PLATFORM="$(detect_platform)"
CARGO_BIN="${ELASTOS_CARGO_BIN:-cargo}"

mkdir -p "$PROOF_ROOT"
PROOF_ROOT="$(cd "$PROOF_ROOT" && pwd -P)"
case "$PROOF_ROOT" in
    /) die "proof root cannot be the filesystem root" ;;
esac
mkdir -p "${PROOF_ROOT}/proof" "${PROOF_ROOT}/inputs"

MODEL_PATH="${ELASTOS_PROOF_MODEL_PATH:-}"
LICENSE_PATH="${ELASTOS_PROOF_LICENSE_PATH:-}"
if [[ "$FETCH" == "1" ]]; then
    [[ -n "$MODEL_PATH" ]] || {
        MODEL_PATH="${PROOF_ROOT}/inputs/$(pinned_field "$MODEL" file)"
        fetch_pinned "$(pinned_field "$MODEL" url)" "$MODEL_PATH"
    }
    [[ -n "$LICENSE_PATH" ]] || {
        LICENSE_PATH="${PROOF_ROOT}/inputs/LICENSE"
        fetch_pinned "$(pinned_field "$MODEL" license_url)" "$LICENSE_PATH"
    }
fi
[[ -n "$MODEL_PATH" && -n "$LICENSE_PATH" ]] \
    || die "set ELASTOS_PROOF_MODEL_PATH and ELASTOS_PROOF_LICENSE_PATH, or pass --fetch"
MODEL_PATH="$(cd "$(dirname "$MODEL_PATH")" && pwd -P)/$(basename "$MODEL_PATH")"
LICENSE_PATH="$(cd "$(dirname "$LICENSE_PATH")" && pwd -P)/$(basename "$LICENSE_PATH")"
verify_pinned_file "model" "$MODEL_PATH" \
    "$(pinned_field "$MODEL" bytes)" "$(pinned_field "$MODEL" sha256)"
verify_pinned_file "license" "$LICENSE_PATH" \
    "$(pinned_field "$MODEL" license_bytes)" "$(pinned_field "$MODEL" license_sha256)"

HOME_ROOT="${PROOF_ROOT}/home"
DATA_DIR="${HOME_ROOT}/xdg/elastos"
if [[ "$PLATFORM" == "darwin-arm64" ]]; then
    DATA_DIR="${HOME_ROOT}/Library/Application Support/elastos"
fi

if [[ "$CHECK_INPUTS" == "1" ]]; then
    cat <<EOF
[pinned-model-proof] plan for ${MODEL} on ${PLATFORM}
  isolated home:     ${HOME_ROOT}
  engine data dir:   ${DATA_DIR}
  native proof:      $(pinned_field "$MODEL" native_test)
  cold proof:        $([[ "$NATIVE_ONLY" == "1" ]] && echo skipped || pinned_field "$MODEL" cold_test)
EOF
    exit 0
fi

need_cmd "$CARGO_BIN"
build_or_reuse() {
    local override="$1" manifest="$2" bin="$3"
    if [[ -n "$override" ]]; then
        [[ -x "$override" ]] || die "prebuilt binary is not executable: $override"
        printf '%s\n' "$(cd "$(dirname "$override")" && pwd -P)/$(basename "$override")"
        return
    fi
    "$CARGO_BIN" build --locked --manifest-path "$manifest" --bin "$bin" >/dev/null 2>&1 \
        || die "cargo build failed for $bin ($manifest)"
    local target_dir
    target_dir="$(dirname "$manifest")/target/debug"
    [[ -x "${target_dir}/${bin}" ]] || die "built binary missing: ${target_dir}/${bin}"
    printf '%s\n' "$(cd "$target_dir" && pwd -P)/${bin}"
}

echo "[pinned-model-proof] resolving binaries"
ELASTOS_BIN="$(build_or_reuse "${ELASTOS_PROOF_ELASTOS_BIN:-}" "${ROOT}/elastos/Cargo.toml" elastos)"
MODEL_PROVIDER="$(build_or_reuse "${ELASTOS_PROOF_MODEL_PROVIDER:-}" "${ROOT}/capsules/model-provider/Cargo.toml" model-provider)"
IPFS_PROVIDER=""
KUBO_BIN=""
if [[ "$NATIVE_ONLY" != "1" ]]; then
    IPFS_PROVIDER="$(build_or_reuse "${ELASTOS_PROOF_IPFS_PROVIDER:-}" "${ROOT}/capsules/ipfs-provider/Cargo.toml" ipfs-provider)"
    KUBO_BIN="${PROOF_ROOT}/kubo/bin/kubo"
    if [[ ! -x "$KUBO_BIN" ]]; then
        "${ROOT}/scripts/seed-kubo-cache.sh" "${PROOF_ROOT}/kubo/cache" "${PROOF_ROOT}/kubo" "$PLATFORM"
    fi
fi

# The ordinary installer owns the engine bundle and its v2 receipt. HOME and
# XDG_DATA_HOME select the isolated root; PATH keeps only system tools.
echo "[pinned-model-proof] installing the local model engine through elastos setup"
mkdir -p "${HOME_ROOT}"
env -i HOME="$HOME_ROOT" XDG_DATA_HOME="${HOME_ROOT}/xdg" PATH="/usr/bin:/bin" \
    ELASTOS_QUIET_RUNTIME_NOTICES=1 \
    "$ELASTOS_BIN" setup --profile operator --with llama-server \
    --without shell,localhost-provider,did-provider >"${PROOF_ROOT}/proof/setup.log" 2>&1 \
    || die "elastos setup failed; see ${PROOF_ROOT}/proof/setup.log"
BUNDLE="$(python3 - "${DATA_DIR}/components.json" "$PLATFORM" <<'PY'
import json, sys
manifest = json.load(open(sys.argv[1]))
info = manifest["external"]["llama-server"]["platforms"][sys.argv[2]]
print(info["install_path"])
PY
)"
RECEIPT="${DATA_DIR}/${BUNDLE}/.elastos-engine.json"
[[ -f "$RECEIPT" ]] || die "engine receipt missing: $RECEIPT"
ENGINE_BIN="${DATA_DIR}/${BUNDLE}/llama-server"
[[ -x "$ENGINE_BIN" ]] || die "engine executable missing: $ENGINE_BIN"

PREFIX="$(pinned_field "$MODEL" env_prefix)"
PROOF_ENV=(
    "ELASTOS_TEST_${PREFIX}_PATH=${MODEL_PATH}"
    "ELASTOS_TEST_${PREFIX}_LICENSE_PATH=${LICENSE_PATH}"
    "ELASTOS_TEST_${PREFIX}_ENGINE_DATA=${DATA_DIR}"
    "ELASTOS_TEST_MODEL_PROVIDER_PATH=${MODEL_PROVIDER}"
)
[[ -z "$IPFS_PROVIDER" ]] || PROOF_ENV+=("ELASTOS_TEST_IPFS_PROVIDER_PATH=${IPFS_PROVIDER}")
[[ -z "$KUBO_BIN" ]] || PROOF_ENV+=("ELASTOS_TEST_KUBO_PATH=${KUBO_BIN}")
run_proof() {
    local test_name="$1" log="${PROOF_ROOT}/proof/${1}.log"
    local test_id="${PROOF_MODULE}::${test_name}"
    echo "[pinned-model-proof] running ${test_id}"
    local status=0
    (
        cd "${ROOT}/elastos"
        env "${PROOF_ENV[@]}" \
            "$CARGO_BIN" test --locked -p elastos-server --lib -- --ignored --nocapture \
            --exact "$test_id"
    ) >"$log" 2>&1 || status=$?
    if [[ "$status" -ne 0 ]]; then
        echo "[pinned-model-proof] ${test_name} FAILED (exit ${status}); see ${log}" >&2
        return "$status"
    fi
    local reason
    if ! reason="$(verify_proof_output "$log" "$test_id")"; then
        echo "[pinned-model-proof] ${test_name} NOT PROVEN: ${reason}; see ${log}" >&2
        return 1
    fi
    echo "[pinned-model-proof] ${test_name} passed"
}

# The verifier that decides "passed" proves itself before any proof runs.
verifier_self_test
RESULT_NATIVE="failed"
RESULT_COLD="skipped"
run_proof "$(pinned_field "$MODEL" native_test)" && RESULT_NATIVE="passed"
if [[ "$NATIVE_ONLY" != "1" ]]; then
    RESULT_COLD="failed"
    run_proof "$(pinned_field "$MODEL" cold_test)" && RESULT_COLD="passed"
fi

SOURCE_COMMIT="$(git -C "$ROOT" rev-parse HEAD)"
SOURCE_TREE="$(git -C "$ROOT" rev-parse 'HEAD^{tree}')"
SOURCE_DIRTY="False"
[[ -z "$(git -C "$ROOT" status --porcelain)" ]] || SOURCE_DIRTY="True"
# A dirty tree means the proof ran against source that no commit identifies.
# The operator names every temporary accommodation so the receipt says which
# behavior came from the reviewed commit and which did not.
ACCOMMODATIONS="${ELASTOS_PROOF_ACCOMMODATIONS:-}"
if [[ "$SOURCE_DIRTY" == "True" && -z "$ACCOMMODATIONS" ]]; then
    die "source tree is dirty; set ELASTOS_PROOF_ACCOMMODATIONS to name the temporary changes"
fi
python3 - "${PROOF_ROOT}/proof/receipt.json" "${ACCOMMODATIONS:-none}" <<PY
import json, sys, time
receipt = {
    "schema": "elastos.pinned-model-consumer-proof/v1",
    "model": "${MODEL}",
    "platform": "${PLATFORM}",
    "recorded_at": int(time.time()),
    "source": {
        "commit": "${SOURCE_COMMIT}",
        "tree": "${SOURCE_TREE}",
        "dirty": ${SOURCE_DIRTY},
        "accommodations": sys.argv[2],
    },
    "inputs": {
        "model_sha256": "$(pinned_field "$MODEL" sha256)",
        "model_bytes": $(pinned_field "$MODEL" bytes),
        "license_sha256": "$(pinned_field "$MODEL" license_sha256)",
    },
    "binaries": {
        "elastos": "$(sha256_file "$ELASTOS_BIN")",
        "model_provider": "$(sha256_file "$MODEL_PROVIDER")",
        "ipfs_provider": "$([[ -n "$IPFS_PROVIDER" ]] && sha256_file "$IPFS_PROVIDER" || echo unused)",
        "kubo": "$([[ -n "$KUBO_BIN" ]] && sha256_file "$KUBO_BIN" || echo unused)",
    },
    "engine": {
        "bundle": "${BUNDLE}",
        "receipt_sha256": "$(sha256_file "$RECEIPT")",
        "executable_sha256": "$(sha256_file "$ENGINE_BIN")",
    },
    "results": {"native_lifecycle": "${RESULT_NATIVE}", "cold_reply_restart": "${RESULT_COLD}"},
    "tests": {
        "native_lifecycle": "${PROOF_MODULE}::$(pinned_field "$MODEL" native_test)",
        "cold_reply_restart": "${PROOF_MODULE}::$(pinned_field "$MODEL" cold_test)",
        "verification": "exact test identity executed once and counted as the single pass",
    },
    "limits": "Isolated test signing key and caller-supplied model bytes; engine activation proof on this host only, not Marketplace delivery evidence.",
}
with open(sys.argv[1], "w") as out:
    json.dump(receipt, out, indent=2, sort_keys=True)
print(json.dumps(receipt, indent=2, sort_keys=True))
PY

[[ "$RESULT_NATIVE" == "passed" && "$RESULT_COLD" != "failed" ]] \
    || die "proof incomplete; receipt and logs are under ${PROOF_ROOT}/proof"
echo "[pinned-model-proof] receipt written to ${PROOF_ROOT}/proof/receipt.json"
