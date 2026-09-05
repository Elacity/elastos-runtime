#!/usr/bin/env bash
#
# Fetch one pinned local-model candidate and its llama.cpp engine.
#
# Artifact URLs, versions, checksums, and install paths come from components.json.
# Model bytes stay below the operator-owned Runtime data root and outside Git.
#
# Usage:
#   ./scripts/fetch/fetch-model.sh                 # Stable candidate
#   ./scripts/fetch/fetch-model.sh experimental    # Low-memory comparison
#   ./scripts/fetch/fetch-model.sh --list

set -euo pipefail
umask 077

BOLD='\033[1m'
GREEN='\033[0;32m'
CYAN='\033[0;36m'
YELLOW='\033[0;33m'
RED='\033[0;31m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
MANIFEST="${ELASTOS_COMPONENTS_MANIFEST:-${PROJECT_ROOT}/components.json}"
INSTALL_DIR="${ELASTOS_DATA_DIR:-${HOME}/.local/share/elastos}"
BIN_DIR="${INSTALL_DIR}/bin"
LLAMA_COMPONENT="llama-server"

cleanup_paths=("")

die()  { echo -e "${RED}Error:${NC} $*" >&2; exit 1; }
info() { echo -e "  ${CYAN}▶${NC} $*"; }
ok()   { echo -e "  ${GREEN}✓${NC} $*"; }
warn() { echo -e "  ${YELLOW}!${NC} $*"; }

cleanup() {
    local path
    for path in "${cleanup_paths[@]}"; do
        [[ -n "$path" ]] && rm -rf -- "$path"
    done
    return 0
}
trap cleanup EXIT HUP INT TERM

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || die "'$1' is required."
}

sha256_file() {
    local path="$1"
    if command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$path" | awk '{print $1}'
    elif command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$path" | awk '{print $1}'
    else
        die "shasum or sha256sum is required."
    fi
}

verify_sha256() {
    local path="$1" expected="$2" actual
    if [[ ! "$expected" =~ ^sha256:[0-9a-f]{64}$ ]]; then
        echo "Invalid SHA-256 declaration for $(basename "$path")." >&2
        return 1
    fi
    actual="$(sha256_file "$path")"
    if [[ "sha256:${actual}" != "$expected" ]]; then
        echo "SHA-256 verification failed for $(basename "$path")." >&2
        return 1
    fi
}

detect_platform() {
    local os arch
    os="$(uname -s)"
    arch="$(uname -m)"
    case "${os}:${arch}" in
        Darwin:arm64) printf '%s\n' darwin-arm64 ;;
        Linux:x86_64) printf '%s\n' linux-amd64 ;;
        Linux:aarch64|Linux:arm64) printf '%s\n' linux-arm64 ;;
        *) die "Unsupported model bootstrap platform: ${os} ${arch}." ;;
    esac
}

component_value() {
    local component="$1" platform="$2" field="$3"
    python3 - "$MANIFEST" "$component" "$platform" "$field" <<'PY'
import json
import pathlib
import sys

manifest_path, component_name, platform, field = sys.argv[1:]
try:
    manifest = json.loads(pathlib.Path(manifest_path).read_text(encoding="utf-8"))
    component = manifest["external"][component_name]
    platform_info = component.get("platforms", {}).get(platform)
    if platform_info is None:
        platform_info = component.get("platforms", {}).get("*", {})
    value = platform_info.get(field, component.get(field))
    if not isinstance(value, (str, int)):
        raise ValueError(f"{component_name}.{platform}.{field} is missing or invalid")
except (KeyError, OSError, ValueError, json.JSONDecodeError) as error:
    raise SystemExit(f"invalid component manifest: {error}")
print(value)
PY
}

validate_relative_path() {
    python3 - "$1" <<'PY'
import pathlib
import sys

path = pathlib.PurePosixPath(sys.argv[1])
if path.is_absolute() or not path.parts or ".." in path.parts:
    raise SystemExit("component install path must be a non-empty relative path")
PY
}

validate_download_url() {
    local url="$1"
    case "$url" in
        https://*) ;;
        http://127.0.0.1:*|http://localhost:*)
            [[ "${ELASTOS_MODEL_ALLOW_LOCAL_HTTP_FIXTURE:-}" == "1" ]] || die "Model artifacts require HTTPS."
            ;;
        *) die "Model artifacts require an HTTPS URL." ;;
    esac
}

download_verified() {
    local label="$1" url="$2" checksum="$3" destination="$4"
    local parent temporary
    parent="$(dirname "$destination")"
    mkdir -p "$parent"

    if [[ -f "$destination" && ! -L "$destination" ]]; then
        if verify_sha256 "$destination" "$checksum" 2>/dev/null; then
            ok "${label} already present and verified"
            return 0
        fi
        warn "${label} checksum changed; downloading a verified replacement"
    elif [[ -e "$destination" || -L "$destination" ]]; then
        die "${label} destination is not a regular file."
    fi

    validate_download_url "$url"
    temporary="$(mktemp "${parent}/.$(basename "$destination").download.XXXXXX")"
    cleanup_paths+=("$temporary")
    info "Downloading ${label}..."
    curl --fail --location --progress-bar --output "$temporary" "$url" || die "Download failed for ${label}."
    verify_sha256 "$temporary" "$checksum" || die "${label} was not installed."
    chmod 0600 "$temporary"
    mv -f -- "$temporary" "$destination"
    ok "${label} downloaded and verified"
}

engine_receipt_matches() {
    local receipt="$1" binary="$2" version="$3" platform="$4" archive_checksum="$5"
    [[ -f "$receipt" && -f "$binary" && ! -L "$binary" ]] || return 1
    python3 - "$receipt" "$version" "$platform" "$archive_checksum" "sha256:$(sha256_file "$binary")" <<'PY'
import json
import pathlib
import sys

receipt_path, version, platform, archive_checksum, binary_checksum = sys.argv[1:]
try:
    receipt = json.loads(pathlib.Path(receipt_path).read_text(encoding="utf-8"))
except (OSError, json.JSONDecodeError):
    raise SystemExit(1)
expected = {
    "schema": "elastos.local-model-engine/v1",
    "version": version,
    "platform": platform,
    "archive_sha256": archive_checksum,
    "llama_server_sha256": binary_checksum,
}
raise SystemExit(0 if receipt == expected else 1)
PY
}

write_engine_receipt() {
    local receipt="$1" binary="$2" version="$3" platform="$4" archive_checksum="$5"
    python3 - "$receipt" "$version" "$platform" "$archive_checksum" "sha256:$(sha256_file "$binary")" <<'PY'
import json
import os
import pathlib
import sys

receipt_path, version, platform, archive_checksum, binary_checksum = sys.argv[1:]
path = pathlib.Path(receipt_path)
payload = {
    "schema": "elastos.local-model-engine/v1",
    "version": version,
    "platform": platform,
    "archive_sha256": archive_checksum,
    "llama_server_sha256": binary_checksum,
}
path.write_text(json.dumps(payload, sort_keys=True, separators=(",", ":")) + "\n", encoding="utf-8")
os.chmod(path, 0o600)
PY
}

bundle_engine_receipt() {
    local mode="$1" bundle="$2" version="$3" platform="$4" archive_checksum="$5"
    python3 - "$mode" "$bundle" "$version" "$platform" "$archive_checksum" <<'PY'
import hashlib
import json
import os
import pathlib
import posixpath
import stat
import sys

mode, bundle_arg, version, platform, archive_checksum = sys.argv[1:]
bundle = pathlib.Path(bundle_arg)
receipt_path = bundle / ".elastos-engine.json"
entries = []
directories = []
regular_files = []


def reject(message):
    raise ValueError(message)


def file_sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return "sha256:" + digest.hexdigest()


def inventory(directory, prefix=""):
    directories.append(directory)
    for child in os.scandir(directory):
        relative = posixpath.join(prefix, child.name)
        if relative == ".elastos-engine.json":
            continue
        metadata = child.stat(follow_symlinks=False)
        path = pathlib.Path(child.path)
        if stat.S_ISDIR(metadata.st_mode):
            inventory(path, relative)
        elif stat.S_ISREG(metadata.st_mode):
            entries.append(
                {"path": relative, "sha256": file_sha256(path), "type": "file"}
            )
            regular_files.append((path, metadata.st_mode))
        elif stat.S_ISLNK(metadata.st_mode):
            target = os.readlink(path)
            if not target:
                reject(f"symlink {relative} has an empty target")
            target_path = pathlib.PurePosixPath(target)
            if target_path.is_absolute():
                reject(f"symlink {relative} has an absolute target")
            if ".." in target_path.parts:
                reject(f"symlink {relative} has a '..' target")
            normalized = posixpath.normpath(
                posixpath.join(posixpath.dirname(relative), target)
            )
            if (
                normalized == ".."
                or normalized.startswith("../")
                or posixpath.isabs(normalized)
            ):
                reject(f"symlink {relative} leaves the bundle")
            entries.append(
                {"path": relative, "target": target, "type": "symlink"}
            )
        else:
            reject(f"entry {relative} is a special file")


def validate_protection():
    owner = os.getuid()
    for path in directories:
        metadata = os.lstat(path)
        if metadata.st_uid != owner or stat.S_IMODE(metadata.st_mode) != 0o500:
            reject(f"directory {path} has unsafe ownership or mode")
    for path, _ in regular_files:
        metadata = os.lstat(path)
        file_mode = stat.S_IMODE(metadata.st_mode)
        if (
            metadata.st_uid != owner
            or metadata.st_nlink != 1
            or file_mode not in (0o400, 0o500)
        ):
            reject(f"file {path} has unsafe ownership, links, or mode")
    metadata = os.lstat(receipt_path)
    if (
        metadata.st_uid != owner
        or metadata.st_nlink != 1
        or stat.S_IMODE(metadata.st_mode) != 0o400
    ):
        reject("receipt has unsafe ownership, links, or mode")


try:
    bundle_metadata = os.lstat(bundle)
    if not stat.S_ISDIR(bundle_metadata.st_mode):
        reject("bundle is not a directory")
    if mode == "write" and os.path.lexists(receipt_path):
        reject("archive contains the reserved receipt path")
    if mode == "verify":
        receipt_metadata = os.lstat(receipt_path)
        if not stat.S_ISREG(receipt_metadata.st_mode):
            reject("receipt is not a regular file")
    elif mode != "write":
        reject(f"unsupported receipt mode: {mode}")

    inventory(bundle)
    entries.sort(key=lambda entry: entry["path"])
    payload = {
        "schema": "elastos.local-model-engine/v2",
        "version": version,
        "platform": platform,
        "archive_sha256": archive_checksum,
        "entries": entries,
    }

    if mode == "verify":
        validate_protection()
        receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
        if receipt != payload:
            reject("receipt does not match the installed inventory")
    else:
        owner = os.getuid()
        if any(os.lstat(path).st_uid != owner for path in directories):
            reject("bundle directory owner differs from the current user")
        if any(
            os.lstat(path).st_uid != owner or os.lstat(path).st_nlink != 1
            for path, _ in regular_files
        ):
            reject("bundle file owner or link count is unsafe")
        receipt_path.write_text(
            json.dumps(payload, sort_keys=True, separators=(",", ":")) + "\n",
            encoding="utf-8",
        )
        os.chmod(receipt_path, 0o400)
        for path, file_mode in regular_files:
            os.chmod(path, 0o500 if file_mode & 0o111 else 0o400)
        for path in sorted(directories, key=lambda value: len(value.parts), reverse=True):
            os.chmod(path, 0o500)
        validate_protection()
except (OSError, ValueError) as error:
    print(f"invalid llama-server bundle: {error}", file=sys.stderr)
    raise SystemExit(1)
PY
}

install_prebuilt_engine() {
    local platform="$1" version url checksum extract_path install_path binary_path
    local archive engine_dir engine_binary receipt stage staged_binary install_temp link_temp
    local engine_ready=0
    version="$(component_value "$LLAMA_COMPONENT" "$platform" version)"
    url="$(component_value "$LLAMA_COMPONENT" "$platform" url)"
    checksum="$(component_value "$LLAMA_COMPONENT" "$platform" checksum)"
    extract_path="$(component_value "$LLAMA_COMPONENT" "$platform" extract_path)"
    install_path="$(component_value "$LLAMA_COMPONENT" "$platform" install_path)"
    binary_path="$(python3 - "$MANIFEST" "$platform" <<'PY'
import json
import pathlib
import sys

manifest = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
info = manifest["external"]["llama-server"].get("platforms", {}).get(sys.argv[2], {})
value = info.get("binary_path", "")
if not isinstance(value, str):
    raise SystemExit("llama-server binary_path must be a string")
print(value)
PY
)"
    validate_relative_path "$extract_path"
    validate_relative_path "$install_path"

    archive="${INSTALL_DIR}/artifacts/llama-${version}-${platform}.tar.gz"
    if [[ -n "$binary_path" ]]; then
        validate_relative_path "$binary_path"
        [[ "$extract_path" == "llama-${version}" ]] \
            || die "llama-server bundle path does not match its version."
        [[ "$install_path" == "libexec/llama.cpp/${version}/${platform}" ]] \
            || die "llama-server bundle install path does not match its version and platform."
        engine_dir="${INSTALL_DIR}/${install_path}"
        engine_binary="${engine_dir}/${binary_path}"
        receipt="${engine_dir}/.elastos-engine.json"
    else
        engine_dir=""
        engine_binary="${INSTALL_DIR}/${install_path}"
        receipt="${BIN_DIR}/.llama-server-${version}-${platform}.json"
    fi
    mkdir -p "$BIN_DIR"

    if [[ -n "$binary_path" ]]; then
        if [[ -e "$engine_dir" || -L "$engine_dir" ]]; then
            if [[ -d "$engine_dir" && ! -L "$engine_dir" \
                && -f "$engine_binary" && -x "$engine_binary" && ! -L "$engine_binary" ]] \
                && bundle_engine_receipt verify "$engine_dir" "$version" "$platform" "$checksum"; then
                engine_ready=1
            else
                die "Existing llama-server bundle failed verification; keep it for operator review."
            fi
        fi
    elif engine_receipt_matches "$receipt" "$engine_binary" "$version" "$platform" "$checksum"; then
        engine_ready=1
    fi

    if [[ "$engine_ready" == "1" ]]; then
        ok "llama-server ${version} already present and verified"
    else
        download_verified "llama-server ${version}" "$url" "$checksum" "$archive"
        stage="$(mktemp -d "${INSTALL_DIR}/.llama-engine.XXXXXX")"
        cleanup_paths+=("$stage")
        tar -xzf "$archive" -C "$stage"
        staged_binary="${stage}/${extract_path}"
        [[ -n "$binary_path" ]] && staged_binary="${staged_binary}/${binary_path}"
        [[ -f "$staged_binary" && ! -L "$staged_binary" ]] \
            || die "Verified llama-server archive has an invalid binary layout."
        chmod 0700 "$staged_binary"
        if [[ -n "$binary_path" ]]; then
            bundle_engine_receipt write "${stage}/${extract_path}" \
                "$version" "$platform" "$checksum"
            chmod 0700 "${stage}/${extract_path}"
            mkdir -p "$(dirname "$engine_dir")"
            [[ ! -e "$engine_dir" && ! -L "$engine_dir" ]] \
                || die "Existing llama-server bundle failed verification; keep it for operator review."
            mv -- "${stage}/${extract_path}" "$engine_dir"
            chmod 0500 "$engine_dir"
        else
            mkdir -p "$(dirname "$engine_binary")"
            install_temp="$(mktemp "$(dirname "$engine_binary")/.llama-server.install.XXXXXX")"
            cleanup_paths+=("$install_temp")
            install -m 0700 "$staged_binary" "$install_temp"
            mv -f -- "$install_temp" "$engine_binary"
            write_engine_receipt "$receipt" "$engine_binary" "$version" "$platform" "$checksum"
        fi
        ok "llama-server ${version} installed and verified"
    fi

    if [[ -n "$binary_path" ]]; then
        if [[ (-e "${BIN_DIR}/llama-server" || -L "${BIN_DIR}/llama-server") \
            && ! -L "${BIN_DIR}/llama-server" ]]; then
            die "llama-server bundle cannot replace an existing non-link executable."
        fi
        link_temp="${BIN_DIR}/.llama-server.link.$$"
        cleanup_paths+=("$link_temp")
        ln -s "$engine_binary" "$link_temp"
        mv -f -- "$link_temp" "${BIN_DIR}/llama-server"
    fi
}

case "${1:-stable}" in
    --help|-h)
        echo "Usage: ./scripts/fetch/fetch-model.sh [stable|experimental|--list]"
        exit 0
        ;;
    --list)
        echo -e "${BOLD}Local model evaluation candidates:${NC}"
        echo "  stable       Qwen3.5-9B Q4_K_M"
        echo "  experimental PrismML Bonsai 8B Q1_0"
        exit 0
        ;;
    stable) MODEL_COMPONENT="model-qwen3.5-9b" ;;
    experimental) MODEL_COMPONENT="model-bonsai-8b-q1" ;;
    *) die "Unknown candidate '$1'. Use stable, experimental, or --list." ;;
esac

[[ -f "$MANIFEST" ]] || die "components.json is unavailable."
require_cmd python3
require_cmd curl
require_cmd tar

PLATFORM="$(detect_platform)"
MODEL_URL="$(component_value "$MODEL_COMPONENT" "$PLATFORM" url)"
MODEL_CHECKSUM="$(component_value "$MODEL_COMPONENT" "$PLATFORM" checksum)"
MODEL_INSTALL_PATH="$(component_value "$MODEL_COMPONENT" "$PLATFORM" install_path)"
MODEL_DESCRIPTION="$(component_value "$MODEL_COMPONENT" "$PLATFORM" description)"
validate_relative_path "$MODEL_INSTALL_PATH"
[[ "$MODEL_INSTALL_PATH" == models/*.gguf ]] || die "Model install path must be a GGUF below models/."

echo
echo -e "${BOLD}ElastOS local model preparation${NC}"
echo "  Candidate: ${MODEL_DESCRIPTION}"
echo "  Platform:  ${PLATFORM}"
echo

download_verified "$MODEL_COMPONENT" "$MODEL_URL" "$MODEL_CHECKSUM" "${INSTALL_DIR}/${MODEL_INSTALL_PATH}"

LLAMA_STRATEGY="$(python3 - "$MANIFEST" "$PLATFORM" <<'PY'
import json
import pathlib
import sys

manifest = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
info = manifest["external"]["llama-server"].get("platforms", {}).get(sys.argv[2], {})
print(info.get("strategy", "prebuilt"))
PY
)"
if [[ "$LLAMA_STRATEGY" == "source-build" ]]; then
    if [[ -x "${BIN_DIR}/llama-server" ]]; then
        ok "Existing source-built llama-server is available"
    else
        die "This platform requires scripts/build/build-llama-server.sh before model evaluation."
    fi
elif [[ "$LLAMA_STRATEGY" == "prebuilt" ]]; then
    install_prebuilt_engine "$PLATFORM"
else
    die "Unsupported llama-server strategy '${LLAMA_STRATEGY}'."
fi

LLAMA_ENGINE_PATH="$(python3 - "${BIN_DIR}/llama-server" <<'PY'
import pathlib
import sys

path = pathlib.Path(sys.argv[1]).resolve(strict=True)
if not path.is_file():
    raise SystemExit("llama-server does not resolve to a regular file")
print(path)
PY
)"

echo
ok "Local model prerequisites are ready"
echo "  Model:        ${INSTALL_DIR}/${MODEL_INSTALL_PATH}"
echo "  llama-server: ${LLAMA_ENGINE_PATH}"
