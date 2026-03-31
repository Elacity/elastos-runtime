#!/usr/bin/env bash
#
# Fetch cloudflared binary for tunnel-provider packaging.
#
# Installs to:
#   capsules/tunnel-provider/bin/<platform>/cloudflared
#
# Usage:
#   ./scripts/fetch/fetch-cloudflared.sh                    # detect current platform
#   ./scripts/fetch/fetch-cloudflared.sh --platform x86_64-linux
#   ./scripts/fetch/fetch-cloudflared.sh --version 2026.2.1
#   ./scripts/fetch/fetch-cloudflared.sh --all
#   ./scripts/fetch/fetch-cloudflared.sh --help
#

set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/../.."

BOLD='\033[1m'
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[0;33m'
NC='\033[0m'

info() { echo -e "  ${GREEN}▶${NC} $*"; }
warn() { echo -e "  ${YELLOW}!${NC} $*"; }
die()  { echo -e "${RED}Error:${NC} $*" >&2; exit 1; }

show_help() {
    echo ""
    echo -e "${BOLD}Fetch cloudflared for tunnel-provider${NC}"
    echo ""
    echo "Usage:"
    echo "  ./scripts/fetch/fetch-cloudflared.sh [options]"
    echo ""
    echo "Options:"
    echo "  --platform P   Target platform (x86_64-linux | aarch64-linux | x86_64-darwin | aarch64-darwin)"
    echo "  --version V    Release tag (recommended for reproducibility; default: latest)"
    echo "  --all          Fetch all supported platforms"
    echo "  --help         Show this help"
    echo ""
    echo "Output path:"
    echo "  capsules/tunnel-provider/bin/<platform>/cloudflared"
    echo ""
    exit 0
}

require_tools() {
    command -v curl >/dev/null 2>&1 || die "curl not found"
    command -v jq >/dev/null 2>&1 || die "jq not found"
    command -v tar >/dev/null 2>&1 || die "tar not found"
    command -v sha256sum >/dev/null 2>&1 || command -v shasum >/dev/null 2>&1 \
        || die "sha256sum or shasum not found (needed for checksum verification)"
}

sha256_of_file() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

detect_platform() {
    local os arch
    os=$(uname -s | tr '[:upper:]' '[:lower:]')
    arch=$(uname -m)
    case "$arch" in
        x86_64|amd64) arch="x86_64" ;;
        aarch64|arm64) arch="aarch64" ;;
        *) die "Unsupported architecture: ${arch}" ;;
    esac
    case "$os" in
        linux)  echo "${arch}-linux" ;;
        darwin) echo "${arch}-darwin" ;;
        *) die "Unsupported OS: ${os}" ;;
    esac
}

asset_name_for_platform() {
    local platform="$1"
    case "$platform" in
        x86_64-linux)  echo "cloudflared-linux-amd64" ;;
        aarch64-linux) echo "cloudflared-linux-arm64" ;;
        x86_64-darwin) echo "cloudflared-darwin-amd64.tgz" ;;
        aarch64-darwin) echo "cloudflared-darwin-arm64.tgz" ;;
        *) die "Unsupported platform: ${platform}" ;;
    esac
}

api_url_for_version() {
    local version="$1"
    if [[ -z "$version" ]]; then
        echo "https://api.github.com/repos/cloudflare/cloudflared/releases/latest"
    else
        echo "https://api.github.com/repos/cloudflare/cloudflared/releases/tags/${version}"
    fi
}

fetch_one() {
    local platform="$1"
    local version="$2"
    local asset_name api_url release_json download_url tag out_dir tmp tmp_extract

    asset_name=$(asset_name_for_platform "$platform")
    api_url=$(api_url_for_version "$version")

    info "Resolving cloudflared release metadata (${platform})..."
    release_json=$(curl -fsSL "$api_url") || die "Failed to fetch release metadata from GitHub API"
    tag=$(echo "$release_json" | jq -r '.tag_name')

    download_url=$(echo "$release_json" | jq -r --arg name "$asset_name" '.assets[] | select(.name == $name) | .browser_download_url' | head -1)
    if [[ -z "$download_url" || "$download_url" == "null" ]]; then
        die "Asset not found for ${platform}: ${asset_name} (tag: ${tag})"
    fi

    # Look for SHA256SUMS asset for checksum verification
    local sums_url
    sums_url=$(echo "$release_json" | jq -r '.assets[] | select(.name | endswith("SHA256SUMS")) | .browser_download_url' | head -1)

    out_dir="capsules/tunnel-provider/bin/${platform}"
    mkdir -p "$out_dir"

    tmp=$(mktemp)
    tmp_extract="$(mktemp -d)"
    trap 'rm -f "$tmp"; rm -rf "$tmp_extract"' RETURN

    info "Downloading ${asset_name} (${tag})..."
    curl -fL --retry 3 --retry-delay 1 -o "$tmp" "$download_url" \
        || die "Failed to download ${asset_name}"

    # SHA256 verification against upstream checksums
    if [[ -n "$sums_url" && "$sums_url" != "null" ]]; then
        info "Verifying SHA256 checksum..."
        local sums_file actual_hash expected_hash
        sums_file=$(mktemp)
        curl -fsSL -o "$sums_file" "$sums_url" \
            || die "Failed to download SHA256SUMS file"
        actual_hash=$(sha256_of_file "$tmp")
        expected_hash=$(grep -F "$asset_name" "$sums_file" | awk '{print $1}' | head -1 || true)
        rm -f "$sums_file"
        if [[ -z "$expected_hash" ]]; then
            warn "No checksum entry for ${asset_name} in SHA256SUMS — skipping verification"
        elif [[ "$actual_hash" != "$expected_hash" ]]; then
            die "SHA256 mismatch for ${asset_name}!\n  Expected: ${expected_hash}\n  Actual:   ${actual_hash}"
        else
            info "SHA256 verified: ${actual_hash}"
        fi
    else
        warn "No SHA256SUMS asset in release ${tag} — skipping checksum verification"
    fi

    if [[ "$asset_name" == *.tgz ]]; then
        tar -xzf "$tmp" -C "$tmp_extract" || die "Failed to extract ${asset_name}"
        [[ -f "$tmp_extract/cloudflared" ]] || die "Extracted archive missing cloudflared binary"
        cp "$tmp_extract/cloudflared" "${out_dir}/cloudflared"
    else
        cp "$tmp" "${out_dir}/cloudflared"
    fi

    chmod +x "${out_dir}/cloudflared"

    info "Installed: ${out_dir}/cloudflared"
    "${out_dir}/cloudflared" --version || warn "Installed binary did not return version cleanly"
}

PLATFORM=""
VERSION=""
FETCH_ALL=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --help|-h) show_help ;;
        --platform)
            [[ -z "${2:-}" ]] && die "Usage: --platform <platform>"
            PLATFORM="$2"
            shift 2
            ;;
        --version)
            [[ -z "${2:-}" ]] && die "Usage: --version <tag-without-v>"
            VERSION="$2"
            shift 2
            ;;
        --all)
            FETCH_ALL=true
            shift
            ;;
        *)
            die "Unknown option: $1"
            ;;
    esac
done

require_tools

echo ""
echo -e "${BOLD}Fetch cloudflared${NC}"
echo ""

if [[ -z "$VERSION" ]]; then
    warn "No --version specified; fetching latest release."
    warn "For reproducible builds, pin a version: --version 2026.2.1"
fi

if [[ "$FETCH_ALL" == true ]]; then
    for p in x86_64-linux aarch64-linux x86_64-darwin aarch64-darwin; do
        fetch_one "$p" "$VERSION"
    done
else
    if [[ -z "$PLATFORM" ]]; then
        PLATFORM="$(detect_platform)"
    fi
    fetch_one "$PLATFORM" "$VERSION"
fi

echo ""
echo -e "${GREEN}Done.${NC}"
echo "Bundled binaries now available under capsules/tunnel-provider/bin/"
echo ""
