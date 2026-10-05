#!/usr/bin/env bash
# Operator Mac steps between the key-free Release package run and the seed.
#
#   scripts/release-publish.sh prepare RUN_ID VERSION
#       Download and verify the run's Mac inputs, then write unsigned signing
#       input for VERSION (the run's install or update version).
#   scripts/release-publish.sh policy VERSION SIGNER KEY OPENSSL
#       Write the exact signer policy for that unsigned input and print the
#       signing command. SIGNER is the installed release-signer.py copy.
#   scripts/release-publish.sh seed VERSION SIGNED_DIR
#       Print the seed import sequence for that signed set. It uses the seed's
#       installed Runtime and provider.
#   scripts/release-publish.sh seed-upgrade RUN_ID
#       Rarely: print the sequence that installs the run's Linux seed package
#       (Runtime, IPFS provider and its source) on the seed.
#
# Environment: RELEASE_SEED (ssh host) and RELEASE_SEED_DATA (seed service data
# dir) for prepare; seed also needs RELEASE_SEED_UNIT (service unit),
# RELEASE_SEED_STAGE (seed staging dir) and RELEASE_SEED_RUNTIME (installed
# Runtime path); seed-upgrade also needs RELEASE_SEED_USER (service user).
# RELEASE_WORK defaults to ~/.local/share/elastos-release. RELEASE_ORIGIN
# defaults to the public origin. See docs/VERSIONING.md "Publishing a release".
set -euo pipefail
umask 077

REPO=Elacity/elastos-runtime
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ORIGIN="${RELEASE_ORIGIN:-https://elastos.elacitylabs.com}"
WORK_ROOT="${RELEASE_WORK:-$HOME/.local/share/elastos-release}"
SOURCE_ROOT=/opt/elastos/release-source

die() { echo "release-publish: $*" >&2; exit 1; }
need_env() { local name; for name; do [[ -n "${!name:-}" ]] || die "set $name"; done; }
sha() { shasum -a 256 "$1" | cut -d ' ' -f 1; }

# Checks the run (Release package, success) and prints "id digest commit" of its
# newest live PREFIX-<commit>-<attempt> artifact after checking commit is on develop.
checked_artifact() {
    local run="$1" prefix="$2" found commit
    [[ "$(gh api "repos/$REPO/actions/runs/$run" --jq '[.path, .conclusion] | join(" ")')" \
        == ".github/workflows/release-package.yml success" ]] || die "run $run is not a successful Release package run"
    found=$(gh api "repos/$REPO/actions/runs/$run/artifacts?per_page=100" --jq "[.artifacts[]
        | select((.name | test(\"^$prefix-[0-9a-f]{40}-[0-9]+\$\")) and (.expired | not))] | sort_by(.id)
        | if length > 0 then \"\(last.id) \(last.digest | ltrimstr(\"sha256:\")) \(last.name)\" else error(\"no $prefix artifact\") end")
    commit=${found##* $prefix-} commit=${commit%-*}
    [[ "$(gh api "repos/$REPO/compare/$commit...develop" --jq .status)" =~ ^(ahead|identical)$ ]] \
        || die "source $commit is not on develop"
    echo "${found% *} $commit"
}

# Prints the seed-side preamble and get NAME ID DIGEST: download, check, unpack.
seed_preamble() {
    cat <<EOF
# On the seed as the service user, with GH_TOKEN allowed to read Actions artifacts:
set -euo pipefail; umask 077; install -d -m 700 $1; cd $1
get() { curl -fsSL -H "Authorization: Bearer \$GH_TOKEN" -o "\$1.zip" "https://api.github.com/repos/$REPO/actions/artifacts/\$2/zip"
        echo "\$3  \$1.zip" | sha256sum -c --quiet; unzip -q "\$1.zip" -d "\$1"; (cd "\$1" && sha256sum -c --quiet SHA256SUMS); }
EOF
}

# Work dir of a prepared version; holds run.json, inputs/, source/, unsigned/.
work_dir() { printf '%s/%s\n' "$WORK_ROOT" "$1"; }

prepare() {
    [[ $# == 2 && "$1" =~ ^[1-9][0-9]*$ ]] || die "usage: prepare RUN_ID VERSION"
    local run="$1" version="$2" work id digest name commit tree did state
    need_env RELEASE_SEED RELEASE_SEED_DATA
    work=$(work_dir "$version")
    [[ ! -e "$work" ]] || die "$work exists; remove it to prepare again"
    mkdir -p "$work/state" "$work/data"
    id=$(checked_artifact "$run" release-mac)
    read -r id digest _ <<< "$id"
    gh api "repos/$REPO/actions/artifacts/$id/zip" > "$work/mac.zip"
    [[ "$(sha "$work/mac.zip")" == "$digest" ]] || die "Mac artifact digest differs"
    unzip -q "$work/mac.zip" -d "$work/parts"
    rm "$work/mac.zip"
    (cd "$work/parts" && shasum -a 256 -c --quiet SHA256SUMS)
    cat "$work/parts"/release-inputs.tar.gz.part* | tar -xzf - -C "$work"
    rm -r "$work/parts"
    (cd "$work/inputs" && shasum -a 256 -c --quiet SHA256SUMS)
    for name in N N1; do
        [[ "$(jq -r .version "$work/inputs/$name/platform-input.json")" == "$version" ]] && break
        [[ "$name" == N1 ]] && die "run $run built neither input for $version"
    done
    commit=$(jq -r .source.commit "$work/inputs/$name/platform-input.json")
    tree=$(jq -r .source.tree "$work/inputs/$name/platform-input.json")
    git -C "$ROOT" fetch -q origin "$commit"
    git -C "$ROOT" worktree add -q --detach "$work/source" "$commit"
    [[ "$(git -C "$work/source" rev-parse 'HEAD^{tree}')" == "$tree" ]] || die "source tree differs"
    (cd "$work/source" && python3 -I -S scripts/release-platform-input.py verify "$work/inputs/$name" > /dev/null)
    tar -xzf "$work/inputs/$name/artifacts/kubo-darwin-arm64.tar.gz" -C "$work" kubo/ipfs
    curl -fsS --max-time 20 "$ORIGIN/.well-known/elastos/carrier-bootstrap.json?role=publisher" > "$work/bootstrap.json"
    did=$(curl -fsS --max-time 20 "$ORIGIN/release-head.json" | jq -er .signer_did)
    state="$RELEASE_SEED_DATA/ElastOS/SystemServices/Publisher/publish-state.json"
    scp -q "$RELEASE_SEED:$state" "$work/state/publish-state.json"
    jq -n --arg run "$run" --arg name "$name" '{run: $run, input: $name}' > "$work/run.json"
    (cd "$work/source" && env ELASTOS_DATA_DIR="$work/data" ELASTOS_PUBLISH_STATE_DIR="$work/state" \
        ELASTOS_IPFS_KUBO_PATH="$work/kubo/ipfs" \
        ELASTOS_SOURCE_CONNECT_TICKET="$(jq -er .ticket "$work/bootstrap.json")" \
        ELASTOS_PUBLISHER_NODE_ID="$(jq -er .node_id "$work/bootstrap.json")" \
        scripts/publish-release.sh --version "$version" --channel canary \
        --prepare-only "$work/unsigned" --publisher-did "$did" \
        --platform-input "aarch64-darwin=$work/inputs/$name" --preview-platform aarch64-darwin)
    echo "Unsigned input: $work/unsigned (signing-input.json sha256 $(sha "$work/unsigned/signing-input.json"))"
}

policy() {
    [[ $# == 4 ]] || die "usage: policy VERSION SIGNER KEY OPENSSL"
    local work input python signer key openssl develop
    work=$(work_dir "$1") input="$(work_dir "$1")/unsigned"
    [[ -f "$input/signing-input.json" ]] || die "prepare $1 first"
    python=$(python3 -c 'import os, sys; print(os.path.realpath(sys.executable))')
    signer=$(realpath "$2") key=$(realpath "$3") openssl=$(realpath "$4")
    [[ "$signer" == "$2" && "$key" == "$3" ]] || die "SIGNER and KEY must be canonical absolute paths"
    develop=$(gh api "repos/$REPO/git/ref/heads/develop" --jq .object.sha)
    jq -n --slurpfile manifest "$input/signing-input.json" --arg repo "$REPO" --arg develop "$develop" \
        --arg manifest_sha256 "$(sha "$input/signing-input.json")" --arg key "$key" \
        --arg signer "$signer" --arg signer_sha256 "$(sha "$signer")" \
        --arg python "$python" --arg python_sha256 "$(sha "$python")" \
        --arg openssl "$openssl" --arg openssl_sha256 "$(sha "$openssl")" '
        $manifest[0] as $m | [$m.files[].size] as $sizes |
        {repository: $repo, commit: $m.source.commit, tree: $m.source.tree,
         version: $m.version, channel: $m.channel, develop_oid: $develop,
         publisher_did: $m.installer.stamps.MAINTAINER_DID, manifest_sha256: $manifest_sha256,
         tool: {path: $signer, sha256: $signer_sha256}, python: {path: $python, sha256: $python_sha256},
         openssl: {path: $openssl, sha256: $openssl_sha256}, key_path: $key,
         max_file_bytes: ($sizes | max), max_snapshot_bytes: ($sizes | add)}' > "$work/policy.json"
    echo "Policy: $work/policy.json (sha256 $(sha "$work/policy.json")). Sign, typing the DID when asked:"
    echo "env -i '$python' -I -S '$signer' --policy '$work/policy.json' --input-root '$input' --output-root '$work/signed'"
}

seed() {
    [[ $# == 2 ]] || die "usage: seed VERSION SIGNED_DIR"
    need_env RELEASE_SEED RELEASE_SEED_DATA RELEASE_SEED_UNIT RELEASE_SEED_STAGE RELEASE_SEED_RUNTIME
    local version="$1" signed work run name did mac mac_digest stage
    signed=$(realpath "$2") work=$(work_dir "$version")
    run=$(jq -er .run "$work/run.json")
    name=$(jq -er .input "$work/run.json")
    did=$(jq -er .signer_did "$signed/release-head.json")
    mac=$(checked_artifact "$run" release-mac)
    read -r mac mac_digest _ <<< "$mac"
    stage="$RELEASE_SEED_STAGE/$version"
    (cd "$signed" && shasum -a 256 -- *) > "$work/signed.SHA256SUMS"
    cat <<EOF
# On this Mac: copy the four small signed files and the signed hash list.
ssh $RELEASE_SEED 'install -d -m 700 $stage/signed'
scp $signed/{install.sh,release.json,release-head.json,components-aarch64-darwin.json} $RELEASE_SEED:$stage/signed/
scp $work/signed.SHA256SUMS $RELEASE_SEED:$stage/

EOF
    seed_preamble "$stage"
    cat <<EOF
get mac $mac $mac_digest
cat mac/release-inputs.tar.gz.part* | tar -xzf -
(cd inputs && sha256sum -c --quiet SHA256SUMS)
cut -c67- signed.SHA256SUMS | while read -r f; do [ -e "signed/\$f" ] || cp "inputs/$name/artifacts/\$f" signed/; done
(cd signed && sha256sum -c --quiet ../signed.SHA256SUMS)
[ "\$(ls signed | wc -l)" = "\$(wc -l < signed.SHA256SUMS)" ]
export ELASTOS_DATA_DIR=$RELEASE_SEED_DATA
publish() { $RELEASE_SEED_RUNTIME publish-release --version $version --channel canary --signed-publication signed \\
            --publisher-did $did --ipfs-provider-bin $RELEASE_SEED_DATA/bin/ipfs-provider "\$@"; }
sudo -v; [ -x $RELEASE_SEED_RUNTIME ]; [ -x $RELEASE_SEED_DATA/bin/ipfs-provider ]; [ -w $RELEASE_SEED_DATA ]
sudo systemctl stop $RELEASE_SEED_UNIT
trap 'sudo systemctl start $RELEASE_SEED_UNIT' EXIT  # the seed is never left down
publish --preflight-only
# Import ends with a known gossip error after commit while the service is stopped.
publish 2>&1 | tee import.log || grep -q 'committed; retry publication to announce its head: No running runtime found' import.log
trap - EXIT; sudo systemctl start $RELEASE_SEED_UNIT
EOF
}

seed_upgrade() {
    [[ $# == 1 && "$1" =~ ^[1-9][0-9]*$ ]] || die "usage: seed-upgrade RUN_ID"
    need_env RELEASE_SEED_USER RELEASE_SEED_DATA RELEASE_SEED_UNIT RELEASE_SEED_STAGE RELEASE_SEED_RUNTIME
    local pkg id digest commit
    pkg=$(checked_artifact "$1" release-seed)
    read -r id digest commit <<< "$pkg"
    seed_preamble "$RELEASE_SEED_STAGE/seed-upgrade-$1"
    cat <<EOF
sudo install -d -o $RELEASE_SEED_USER /opt/elastos
get seed $id $digest
chmod 755 seed/elastos seed/ipfs-provider
rm -rf $SOURCE_ROOT.new; mkdir $SOURCE_ROOT.new; tar -xzf seed/source.tar.gz -C $SOURCE_ROOT.new
[ "\$(git -C $SOURCE_ROOT.new rev-parse HEAD)" = $commit ]
[ -z "\$(git -C $SOURCE_ROOT.new status --porcelain)" ]
jq --arg c "sha256:\$(sha256sum seed/ipfs-provider | cut -d ' ' -f 1)" --argjson s "\$(stat -c %s seed/ipfs-provider)" \\
   '.external["ipfs-provider"].platforms["linux-amd64"] += {checksum: \$c, size: \$s}' \\
   $RELEASE_SEED_DATA/components.json > components.json
sudo -v; [ -w /opt/elastos ]; [ -w \$(dirname $RELEASE_SEED_RUNTIME) ]; [ -w $RELEASE_SEED_DATA/bin ]; [ -w $RELEASE_SEED_DATA ]
export ELASTOS_DATA_DIR=$RELEASE_SEED_DATA
sudo systemctl stop $RELEASE_SEED_UNIT
trap 'sudo systemctl start $RELEASE_SEED_UNIT' EXIT  # the seed is never left down
rm -rf $SOURCE_ROOT; mv $SOURCE_ROOT.new $SOURCE_ROOT
install -m 755 seed/elastos $RELEASE_SEED_RUNTIME
install -m 755 seed/ipfs-provider $RELEASE_SEED_DATA/bin/ipfs-provider
install -m 600 components.json $RELEASE_SEED_DATA/components.json
$SOURCE_ROOT/scripts/installed-provider-verify.sh --require-verified ipfs-provider
trap - EXIT; sudo systemctl start $RELEASE_SEED_UNIT
EOF
}

command="${1:-}"
case "$command" in
    prepare|policy|seed) shift; "$command" "$@" ;;
    seed-upgrade) shift; seed_upgrade "$@" ;;
    *) sed -n '2,22p' "${BASH_SOURCE[0]}"; exit 2 ;;
esac
