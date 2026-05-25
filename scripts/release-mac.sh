#!/usr/bin/env bash
# scripts/release-mac.sh — Phase 6 Day 4 (Sub-3).
#
# Sign + notarize + staple the Mac `elastos-server` binary using the
# operator's Apple Developer-ID Application certificate. Sibling to
# `scripts/publish-release.sh` (which handles the cross-platform
# IPFS-publish flow) — this script is darwin-only.
#
# This recipe is part of the **operator handoff** for Phase 6 Day 4b:
# the script is shipped here for reproducibility; the operator runs it
# on a dev Mac with the Developer ID cert installed, then commits the
# signed/notarized binary into the release pipeline.
#
# Anchor: docs/vz-backend/PHASE_3_DAY_7_NOTES.md (entitlement runtime
# check substrate this script signs for); docs/vz-backend/PHASE_6_PLAN.md
# § Day 4.
#
# Prerequisites (operator-side, dev Mac):
#   - Apple Developer Program enrollment (paid; ~$99/yr).
#   - Developer ID Application certificate installed in the login keychain:
#       `security find-identity -v -p codesigning` should list one entry
#       starting with "Developer ID Application: <name> (TEAMID)".
#   - notarytool keychain profile (one-time setup):
#       `xcrun notarytool store-credentials elastos-notarytool \
#           --apple-id you@example.com \
#           --team-id <TEAMID> \
#           --password <app-specific-password>`
#   - The release-mode `elastos-server` binary already built (typically
#       under `elastos/target/release/elastos-server` on the dev Mac).
#
# Inputs (env vars, all required unless defaulted):
#   - ELASTOS_SIGNING_IDENTITY     codesigning identity name from
#                                  `security find-identity -v -p codesigning`.
#                                  Example: "Developer ID Application: Sash Inc. (ABCDE12345)"
#   - ELASTOS_NOTARYTOOL_PROFILE   keychain profile name created by
#                                  `xcrun notarytool store-credentials`.
#                                  Default: "elastos-notarytool".
#   - ELASTOS_RELEASE_BINARY       path to the Mach-O binary to sign.
#                                  Default: ${REPO_ROOT}/elastos/target/release/elastos-server.
#   - ELASTOS_ENTITLEMENTS_PLIST   path to the entitlements plist.
#                                  Default: scripts/release/elastos-server.entitlements.plist.
#
# Flow (operator should run interactively the first time; CI uses the
# same flow under a wrapper that supplies env vars from keychain
# secrets):
#   1. Preflight  — verify cert, profile, binary, plist all present.
#   2. Sign       — codesign with hardened runtime + entitlements.
#   3. Verify     — codesign --verify; spctl --assess.
#   4. Notarize   — xcrun notarytool submit --wait.
#   5. Staple     — xcrun stapler staple.
#   6. Re-verify  — spctl --assess --type execute (must report
#                   "source=Notarized Developer ID").
#   7. Print signed binary path + sha256 for the release manifest.
#
# Exit codes:
#   0    everything green; binary signed, notarized, stapled.
#   1    preflight failed (missing cert / profile / binary / plist).
#   2    codesign step failed.
#   3    spctl pre-notarization verify failed (unsigned or wrong cert).
#   4    notarytool submission rejected by Apple. Full log on stderr.
#   5    stapler step failed.
#   6    post-staple spctl re-verify failed.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

ELASTOS_SIGNING_IDENTITY="${ELASTOS_SIGNING_IDENTITY:-}"
ELASTOS_NOTARYTOOL_PROFILE="${ELASTOS_NOTARYTOOL_PROFILE:-elastos-notarytool}"
ELASTOS_RELEASE_BINARY="${ELASTOS_RELEASE_BINARY:-${REPO_ROOT}/elastos/target/release/elastos-server}"
ELASTOS_ENTITLEMENTS_PLIST="${ELASTOS_ENTITLEMENTS_PLIST:-${REPO_ROOT}/scripts/release/elastos-server.entitlements.plist}"

log()  { printf '[release-mac] %s\n' "$*" >&2; }
die()  { log "ERROR: $*"; exit "${2:-1}"; }

# ── 1. Preflight ───────────────────────────────────────────────────────────
log "preflight"

[[ "$(uname -s)" == "Darwin" ]] \
    || die "this script only runs on macOS (uname=$(uname -s))"

[[ -n "${ELASTOS_SIGNING_IDENTITY}" ]] \
    || die "ELASTOS_SIGNING_IDENTITY env var required. See 'security find-identity -v -p codesigning' for the value; expect 'Developer ID Application: <name> (TEAMID)'."

[[ -f "${ELASTOS_RELEASE_BINARY}" ]] \
    || die "release binary not found: ${ELASTOS_RELEASE_BINARY}. Run 'cargo build --release -p elastos-server' first."

[[ -f "${ELASTOS_ENTITLEMENTS_PLIST}" ]] \
    || die "entitlements plist not found: ${ELASTOS_ENTITLEMENTS_PLIST}"

# Verify the identity actually resolves; bail early with a clear msg.
if ! security find-identity -v -p codesigning 2>/dev/null | grep -F "${ELASTOS_SIGNING_IDENTITY}" >/dev/null; then
    die "signing identity '${ELASTOS_SIGNING_IDENTITY}' not found in keychain. Run 'security find-identity -v -p codesigning' and copy a 'Developer ID Application' entry."
fi
log "signing identity OK: ${ELASTOS_SIGNING_IDENTITY}"

# Verify the notarytool profile exists.
if ! xcrun notarytool history --keychain-profile "${ELASTOS_NOTARYTOOL_PROFILE}" >/dev/null 2>&1; then
    die "notarytool profile '${ELASTOS_NOTARYTOOL_PROFILE}' not found or invalid. Set up via 'xcrun notarytool store-credentials ${ELASTOS_NOTARYTOOL_PROFILE} --apple-id <id> --team-id <TEAMID> --password <app-specific-password>'."
fi
log "notarytool profile OK: ${ELASTOS_NOTARYTOOL_PROFILE}"

log "preflight green; binary=${ELASTOS_RELEASE_BINARY}"

# ── 2. Sign ────────────────────────────────────────────────────────────────
log "codesign (hardened runtime + entitlements)"

codesign --force \
    --sign "${ELASTOS_SIGNING_IDENTITY}" \
    --options runtime \
    --entitlements "${ELASTOS_ENTITLEMENTS_PLIST}" \
    --timestamp \
    "${ELASTOS_RELEASE_BINARY}" \
    || die "codesign failed" 2

log "codesign OK"

# ── 3. Verify pre-notarization ─────────────────────────────────────────────
log "codesign --verify"
codesign --verify --verbose=2 "${ELASTOS_RELEASE_BINARY}" 2>&1 | sed 's/^/  /' \
    || die "codesign --verify failed (binary is unsigned or signature corrupted)" 3

log "spctl --assess (pre-notarization, expect 'source=No Matching Rule')"
spctl --assess --type execute --verbose=2 "${ELASTOS_RELEASE_BINARY}" 2>&1 | sed 's/^/  /' || true
# Note: spctl will REJECT here pre-notarization; the assertive check is
# post-stapler in step 6.

# ── 4. Notarize ────────────────────────────────────────────────────────────
log "notarize via xcrun notarytool submit --wait"
log "(this typically takes 1–5 minutes; Apple's notary service does the work)"

# notarytool requires a zip/dmg/pkg, not a bare Mach-O. Wrap.
TMPDIR_NOTARIZE="$(mktemp -d)"
trap 'rm -rf "${TMPDIR_NOTARIZE}"' EXIT
ZIP_PATH="${TMPDIR_NOTARIZE}/elastos-server-notarize.zip"
ditto -c -k --sequesterRsrc --keepParent "${ELASTOS_RELEASE_BINARY}" "${ZIP_PATH}"

SUBMIT_OUT="${TMPDIR_NOTARIZE}/submit.json"
xcrun notarytool submit "${ZIP_PATH}" \
    --keychain-profile "${ELASTOS_NOTARYTOOL_PROFILE}" \
    --wait \
    --output-format json \
    > "${SUBMIT_OUT}" \
    || die "notarytool submit returned non-zero" 4

STATUS="$(python3 -c "import json; print(json.load(open('${SUBMIT_OUT}'))['status'])")"
if [[ "${STATUS}" != "Accepted" ]]; then
    log "notarization status: ${STATUS}"
    log "fetching detailed log..."
    SUBMIT_ID="$(python3 -c "import json; print(json.load(open('${SUBMIT_OUT}'))['id'])")"
    xcrun notarytool log "${SUBMIT_ID}" \
        --keychain-profile "${ELASTOS_NOTARYTOOL_PROFILE}" >&2 || true
    die "notarization REJECTED (status=${STATUS}). See log above; common causes: unhardened dylibs, missing --options runtime, expired cert." 4
fi
log "notarization Accepted"

# ── 5. Staple ──────────────────────────────────────────────────────────────
log "xcrun stapler staple"
xcrun stapler staple "${ELASTOS_RELEASE_BINARY}" 2>&1 | sed 's/^/  /' \
    || die "stapler staple failed" 5
log "stapler OK"

# ── 6. Post-staple verify ──────────────────────────────────────────────────
log "spctl --assess --type execute (post-notarization, expect 'source=Notarized Developer ID')"
spctl --assess --type execute --verbose=2 "${ELASTOS_RELEASE_BINARY}" 2>&1 | sed 's/^/  /' \
    || die "spctl re-assess failed after staple — notarization ticket missing or invalid" 6
log "post-staple verify OK"

# ── 7. Summary ─────────────────────────────────────────────────────────────
SIGNED_SHA="$(shasum -a 256 "${ELASTOS_RELEASE_BINARY}" | awk '{print $1}')"
SIGNED_SIZE="$(stat -f%z "${ELASTOS_RELEASE_BINARY}" 2>/dev/null || stat -c%s "${ELASTOS_RELEASE_BINARY}")"

cat <<EOF

╔═══════════════════════════════════════════════════════════════════════════
║ elastos-server signed + notarized + stapled
╠═══════════════════════════════════════════════════════════════════════════
║ Binary:    ${ELASTOS_RELEASE_BINARY}
║ Identity:  ${ELASTOS_SIGNING_IDENTITY}
║ sha256:    sha256:${SIGNED_SHA}
║ Size:      ${SIGNED_SIZE} bytes
║ Notarized: yes (status=Accepted, ticket stapled)
║ Hardened:  yes (--options runtime)
║ Entitlements: ${ELASTOS_ENTITLEMENTS_PLIST}
╠═══════════════════════════════════════════════════════════════════════════
║ Sanity checks the operator should now run before commit:
║
║   1. Run the signed binary and exercise a NAT-only capsule:
║      "${ELASTOS_RELEASE_BINARY}" --help
║      (must not be rejected by Gatekeeper.)
║
║   2. Confirm the entitlement check substrate sees the entitlement:
║      RUST_LOG=elastos_vz=info "${ELASTOS_RELEASE_BINARY}" --version
║      (no "lacks com.apple.vm.networking" log line for a signed
║      release binary with the entitlement granted.)
║
║   3. Commit the signed binary into the release pipeline. The
║      Linux-side commit message convention applies:
║        "Phase 6 Day 4b — sign + notarize elastos-server Mac binary"
╚═══════════════════════════════════════════════════════════════════════════
EOF
