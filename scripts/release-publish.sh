#!/usr/bin/env bash
# Operator Mac steps between the key-free Release package run and the seed.
#
#   scripts/release-publish.sh prepare RUN_ID VERSION [PLATFORM ...]
#       Verify the run's native inputs and write unsigned signing input for
#       VERSION. Defaults to all three platforms; include aarch64-darwin.
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
# RELEASE_APPROVAL names a pre-existing operator-owned approval outside RELEASE_WORK.
# It pins source H, trusted builder W/tree/ref, the public delivery tuple and gh.
# Historical packages need a new attested build; this tool creates no approval.
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
artifact_prefix() {
    case "$1" in aarch64-darwin) echo release-mac ;; *) echo "release-$1" ;; esac
}

# Bind each artifact ID/digest/name suffix to the approved builder and source.
provenance() {
    need_env RELEASE_APPROVAL
    python3 -I -S "$ROOT/scripts/release-provenance.py" "$@" \
        --approval "$RELEASE_APPROVAL" --work-root "$WORK_ROOT"
}

checked_artifact() {
    local run="$1" prefix="$2" found commit id digest attempt
    found=$(provenance artifact --run "$run" --prefix "$prefix") || die "package producer refused"
    read -r id digest commit attempt <<< "$found"
    [[ "$(gh api "repos/$REPO/compare/$commit...develop" --jq .status)" =~ ^(ahead|identical)$ ]] \
        || die "source $commit is not on develop"
    echo "$found"
}

# Bind the package source to successful CI on develop or its merge queue.
checked_ci() {
    local commit="$1" ci required jobs refused
    ci=$(gh api "repos/$REPO/actions/workflows/ci.yml/runs?head_sha=$commit&per_page=100" \
        --paginate | jq -s --arg commit "$commit" '[.[].workflow_runs[] | select(.head_sha == $commit
        and .path == ".github/workflows/ci.yml"
        and ((.event == "push" and .head_branch == "develop")
          or (.event == "merge_group" and ((.head_branch // "") | startswith("gh-readonly-queue/develop/"))))
        and .status == "completed" and .conclusion == "success")] | sort_by(.id)
        | last | if . == null then empty else {id, url: .html_url} end') || die "cannot read develop CI runs for $commit"
    [[ -n "$ci" ]] || die "source $commit needs a completed successful develop CI run (push or merge group)"
    required=$(gh api "repos/$REPO/branches/develop/protection" --jq \
        '.required_status_checks.contexts | if type == "array" and length > 0 then . else error("required CI jobs missing") end') \
        || die "cannot read develop required jobs"
    jobs=$(gh api "repos/$REPO/actions/runs/$(jq -r .id <<< "$ci")/jobs?filter=latest&per_page=100" \
        --paginate | jq -s '[.[].jobs[] | {name, status, conclusion}]') || die "cannot read develop CI jobs"
    refused=$(jq -nr --argjson required "$required" --argjson jobs "$jobs" '
        $required[] as $name | [$jobs[] | select(.name == $name)] as $matches |
        if ($matches | length) == 0 then "\($name): missing"
        else $matches[] | select(.status != "completed" or .conclusion != "success") |
            "\($name): \(.conclusion // .status)" end') || die "cannot check develop required jobs"
    [[ -z "$refused" ]] || die "develop CI run $(jq -r .id <<< "$ci") required jobs refused: $refused"
    echo "$ci"
}

# Print the same source and run identities at each operator handoff.
publication_record() {
    local work="$1" commit run ci_id ci_url
    commit=$(jq -er .source.commit "$work/unsigned/signing-input.json")
    run=$(jq -er .run "$work/run.json")
    ci_id=$(jq -er .ci_run.id "$work/run.json")
    ci_url=$(jq -er .ci_run.url "$work/run.json")
    echo "# Source commit: $commit"
    echo "# Package run: $run https://github.com/$REPO/actions/runs/$run"
    echo "# Develop CI run: $ci_id $ci_url"
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
    [[ $# -ge 2 && "$1" =~ ^[1-9][0-9]*$ ]] || die "usage: prepare RUN_ID VERSION [PLATFORM ...]"
    local run="$1" version="$2" work id digest name commit="" tree="" did state platform input artifact_commit pair="" ci_run approval_json run_attempt artifact_attempt proof_id proof_digest proof_attempt receipts='[]'
    shift 2
    local platforms=("$@") args=() seen=" "
    [[ ${#platforms[@]} -gt 0 ]] || platforms=(aarch64-darwin x86_64-linux aarch64-linux)
    for platform in "${platforms[@]}"; do
        case "$platform" in aarch64-darwin|x86_64-linux|aarch64-linux) ;; *) die "unsupported platform: $platform" ;; esac
        [[ "$seen" != *" $platform "* ]] || die "duplicate platform: $platform"
        seen+="$platform "
    done
    [[ "$seen" == *" aarch64-darwin "* ]] || die "operator Mac inputs must include aarch64-darwin"
    need_env RELEASE_SEED RELEASE_SEED_DATA
    approval_json=$(provenance approval) || die "independent approval refused"
    run_attempt=$(provenance run --run "$run") || die "package producer refused"
    work=$(work_dir "$version")
    [[ ! -e "$work" ]] || die "$work exists; remove it to prepare again"
    mkdir -p "$work/state" "$work/data"
    for platform in "${platforms[@]}"; do
        id=$(checked_artifact "$run" "$(artifact_prefix "$platform")")
        read -r id digest artifact_commit artifact_attempt <<< "$id"
        gh api "repos/$REPO/actions/artifacts/$id/zip" > "$work/input.zip"
        [[ "$(sha "$work/input.zip")" == "$digest" ]] || die "$platform artifact digest differs"
        python3 -I -S "$ROOT/scripts/release-provenance-seal.py" unzip --input "$work/input.zip" --output "$work/parts"
        rm "$work/input.zip"
        python3 -I -S "$ROOT/scripts/release-provenance-seal.py" checksums --input "$work/parts" --output "$work/unused"
        input="$work/inputs/$platform"
        python3 -I -S "$ROOT/scripts/release-provenance-seal.py" untar --input "$work/parts" --output "$input"
        rm -r "$work/parts"
        python3 -I -S "$ROOT/scripts/release-provenance-seal.py" checksums --input "$input" --output "$work/unused"
        for name in N N1; do
            [[ "$(jq -r .version "$input/$name/platform-input.json")" == "$version" ]] && break
            [[ "$name" == N1 ]] && die "run $run built neither $platform input for $version"
        done
        [[ -n "$pair" ]] || pair="$name"
        [[ "$name" == "$pair" ]] || die "$platform selected a different N/N1 input"
        [[ -n "$commit" ]] || commit=$(jq -r .source.commit "$input/$name/platform-input.json")
        [[ -n "$tree" ]] || tree=$(jq -r .source.tree "$input/$name/platform-input.json")
        [[ "$artifact_commit" == "$commit" && "$(jq -r '[.source.commit, .source.tree, .platform, .version] | join(" ")' "$input/$name/platform-input.json")" \
            == "$commit $tree $platform $version" ]] || die "$platform input source, tree, platform or version differs"
        read -r proof_id proof_digest _ proof_attempt <<< "$(checked_artifact "$run" "$(artifact_prefix "$platform" | sed 's/^release-/release-proof-/')")"
        gh api "repos/$REPO/actions/artifacts/$proof_id/zip" > "$work/proof.zip"
        [[ "$(sha "$work/proof.zip")" == "$proof_digest" ]] || die "$platform proof digest differs"
        python3 -I -S "$ROOT/scripts/release-provenance-seal.py" unzip --input "$work/proof.zip" --output "$work/proof"
        provenance receipt --run "$run" --prefix "$(artifact_prefix "$platform")" \
            --receipt "$work/proof/$name.platform-input.json" --bundle "$work/proof/provenance.jsonl" \
            --payload-root "$input/$name" --platform "$platform" --version "$version" > /dev/null \
            || die "$platform attested receipt refused"
        cp "$work/proof/$name.platform-input.json" "$input/$name/platform-input.json"
        cp "$work/proof/provenance.jsonl" "$input/provenance.jsonl"
        rm -rf "$work/proof" "$work/proof.zip"
        receipts=$(jq -n --argjson previous "$receipts" --arg platform "$platform" --argjson id "$id" \
            --arg digest "sha256:$digest" --arg name "$(artifact_prefix "$platform")-$commit-$artifact_attempt" --argjson attempt "$artifact_attempt" \
            --argjson proof_id "$proof_id" --arg proof_digest "sha256:$proof_digest" --argjson proof_attempt "$proof_attempt" \
            --arg proof_name "$(artifact_prefix "$platform" | sed 's/^release-/release-proof-/')-$commit-$proof_attempt" \
            '$previous + [{platform: $platform, artifact: {id: $id, digest: $digest, name: $name, attempt: $attempt},
              proof_artifact: {id: $proof_id, digest: $proof_digest, name: $proof_name, attempt: $proof_attempt}}]' )
        args+=(--platform-input "$platform=$input/$name")
    done
    name="$pair"
    ci_run=$(checked_ci "$commit")
    echo "# Develop CI run: $(jq -r '[(.id | tostring), .url] | join(" ")' <<< "$ci_run")"
    [[ ${#platforms[@]} -gt 1 ]] || args+=(--preview-platform aarch64-darwin)
    git -C "$ROOT" fetch -q origin "$commit"
    git -C "$ROOT" worktree add -q --detach "$work/source" "$commit"
    [[ "$(git -C "$work/source" rev-parse 'HEAD^{tree}')" == "$tree" ]] || die "source tree differs"
    for platform in "${platforms[@]}"; do
        (cd "$work/source" && python3 -I -S scripts/release-platform-input.py verify "$work/inputs/$platform/$name" > /dev/null)
    done
    tar -xzf "$work/inputs/aarch64-darwin/$name/artifacts/kubo-darwin-arm64.tar.gz" -C "$work" kubo/ipfs
    curl -fsS --max-time 20 "$ORIGIN/.well-known/elastos/carrier-bootstrap.json?role=publisher" > "$work/bootstrap.json"
    did=$(curl -fsS --max-time 20 "$ORIGIN/release-head.json" | jq -er .signer_did)
    [[ "$ORIGIN" == "$(jq -r .delivery.PUBLISHER_GATEWAY <<< "$approval_json")" \
        && "$did" == "$(jq -r .delivery.MAINTAINER_DID <<< "$approval_json")" \
        && "$(jq -r .node_id "$work/bootstrap.json")" == "$(jq -r .delivery.PUBLISHER_NODE_ID <<< "$approval_json")" \
        && "$(jq -r .ticket "$work/bootstrap.json")" == "$(jq -r .delivery.SOURCE_CONNECT_TICKET <<< "$approval_json")" ]] \
        || die "publisher bootstrap differs from independent delivery approval"
    state="$RELEASE_SEED_DATA/ElastOS/SystemServices/Publisher/publish-state.json"
    scp -q "$RELEASE_SEED:$state" "$work/state/publish-state.json"
    printf '%s\n' "${platforms[@]}" | jq -Rn --arg run "$run" --arg name "$name" --argjson ci_run "$ci_run" \
        --argjson run_attempt "$run_attempt" --arg approval "$RELEASE_APPROVAL" --arg approval_sha256 "$(jq -r .sha256 <<< "$approval_json")" \
        --argjson receipts "$receipts" \
        '{run: $run, run_attempt: $run_attempt, approval: {path: $approval, sha256: $approval_sha256}, receipts: $receipts,
          ci_run: $ci_run, input: $name, platforms: [inputs]}' > "$work/run.json"
    (cd "$work/source" && env ELASTOS_DATA_DIR="$work/data" ELASTOS_PUBLISH_STATE_DIR="$work/state" \
        ELASTOS_IPFS_KUBO_PATH="$work/kubo/ipfs" \
        ELASTOS_SOURCE_CONNECT_TICKET="$(jq -er .ticket "$work/bootstrap.json")" \
        ELASTOS_PUBLISHER_NODE_ID="$(jq -er .node_id "$work/bootstrap.json")" \
        ELASTOS_PUBLISHER_GATEWAY="$(jq -er .delivery.PUBLISHER_GATEWAY <<< "$approval_json")" \
        ELASTOS_IPNS_NAME="$(jq -r .delivery.IPNS_NAME <<< "$approval_json")" \
        scripts/publish-release.sh --version "$version" --channel canary \
        --prepare-only "$work/unsigned" --publisher-did "$did" \
        "${args[@]}")
    provenance bind --work "$work" || die "unsigned input differs from approved source/delivery"
    echo "Unsigned input: $work/unsigned (signing-input.json sha256 $(sha "$work/unsigned/signing-input.json"))"
}

policy() {
    [[ $# == 4 ]] || die "usage: policy VERSION SIGNER KEY OPENSSL"
    local work input python signer key openssl develop approval_json approval_sha256
    work=$(work_dir "$1") input="$(work_dir "$1")/unsigned"
    [[ -f "$input/signing-input.json" ]] || die "prepare $1 first"
    approval_json=$(provenance approval) || die "independent approval refused"
    approval_sha256=$(jq -r .sha256 <<< "$approval_json")
    [[ "$(jq -r '[.approval.path, .approval.sha256] | join(" ")' "$work/run.json")" == "$RELEASE_APPROVAL $approval_sha256" ]] \
        || die "prepared independent approval changed"
    publication_record "$work"
    python=$(python3 -c 'import os, sys; print(os.path.realpath(sys.executable))')
    signer=$(realpath "$2") key=$(realpath "$3") openssl=$(realpath "$4")
    [[ "$signer" == "$2" && "$key" == "$3" ]] || die "SIGNER and KEY must be canonical absolute paths"
    develop=$(gh api "repos/$REPO/git/ref/heads/develop" --jq .object.sha)
    jq -n --slurpfile manifest "$input/signing-input.json" --arg repo "$REPO" --arg develop "$develop" \
        --arg approval "$RELEASE_APPROVAL" --arg approval_sha256 "$approval_sha256" \
        --arg manifest_sha256 "$(sha "$input/signing-input.json")" --arg key "$key" \
        --arg signer "$signer" --arg signer_sha256 "$(sha "$signer")" \
        --arg python "$python" --arg python_sha256 "$(sha "$python")" \
        --arg openssl "$openssl" --arg openssl_sha256 "$(sha "$openssl")" '
        $manifest[0] as $m | [$m.files[].size] as $sizes |
        {repository: $repo, commit: $m.source.commit, tree: $m.source.tree,
         version: $m.version, channel: $m.channel, develop_oid: $develop,
         publisher_did: $m.installer.stamps.MAINTAINER_DID, manifest_sha256: $manifest_sha256,
         approval: {path: $approval, sha256: $approval_sha256},
         tool: {path: $signer, sha256: $signer_sha256}, python: {path: $python, sha256: $python_sha256},
         openssl: {path: $openssl, sha256: $openssl_sha256}, key_path: $key,
         max_file_bytes: ($sizes | max), max_snapshot_bytes: ($sizes | add)}' > "$work/policy.json"
    echo "Policy: $work/policy.json (sha256 $(sha "$work/policy.json")). Sign, typing the DID when asked:"
    echo "env -i GH_TOKEN=\"\$(gh auth token)\" '$python' -I -S '$signer' --policy '$work/policy.json' --input-root '$input' --output-root '$work/signed'"
}

seed() {
    [[ $# == 2 ]] || die "usage: seed VERSION SIGNED_DIR"
    need_env RELEASE_SEED RELEASE_SEED_DATA RELEASE_SEED_UNIT RELEASE_SEED_STAGE RELEASE_SEED_RUNTIME
    local version="$1" signed work run name did platform id digest stage file artifact_commit artifact_attempt candidate recorded local_files=()
    signed=$(realpath "$2") work=$(work_dir "$version")
    run=$(jq -er .run "$work/run.json")
    name=$(jq -er .input "$work/run.json")
    # Keep the prepared input identity through the last operator handoff.
    while read -r platform; do
        read -r id digest artifact_commit artifact_attempt <<< "$(checked_artifact "$run" "$(artifact_prefix "$platform")")"
        candidate=$(jq -n --argjson id "$id" --arg digest "sha256:$digest" --argjson attempt "$artifact_attempt" \
            --arg name "$(artifact_prefix "$platform")-$artifact_commit-$artifact_attempt" \
            '{id: $id, digest: $digest, name: $name, attempt: $attempt}')
        recorded=$(jq -ce --arg platform "$platform" '.receipts[] | select(.platform == $platform) | .artifact' "$work/run.json")
        [[ "$(jq -Sc . <<< "$candidate")" == "$(jq -Sc . <<< "$recorded")" ]] \
            || die "$platform package changed after preparation; prepare and sign the new candidate"
    done < <(jq -er '.platforms[]' "$work/run.json")
    publication_record "$work"
    did=$(jq -er .signer_did "$signed/release-head.json")
    stage="$RELEASE_SEED_STAGE/$version"
    (cd "$signed" && shasum -a 256 -- *) > "$work/signed.SHA256SUMS"
    # The seed takes native artifacts from the run; every other signed file is
    # copied from this Mac, so the seed can serve the whole signed set.
    for file in "$signed"/*; do
        compgen -G "$work/inputs/*/$name/artifacts/${file##*/}" > /dev/null || local_files+=("$file")
    done
    cat <<EOF
# On this Mac: copy every signed file the run does not carry, and the signed hash list.
ssh $RELEASE_SEED 'install -d -m 700 $stage/signed'
scp ${local_files[*]} $RELEASE_SEED:$stage/signed/
scp $work/signed.SHA256SUMS $RELEASE_SEED:$stage/

EOF
    seed_preamble "$stage"
    while read -r platform; do
        id=$(jq -er --arg platform "$platform" '.receipts[] | select(.platform == $platform) | .artifact.id' "$work/run.json")
        digest=$(jq -er --arg platform "$platform" '.receipts[] | select(.platform == $platform) | .artifact.digest | sub("^sha256:"; "")' "$work/run.json")
        cat <<EOF
get $platform $id $digest
mkdir -p inputs/$platform
cat $platform/release-inputs.tar.gz.part* | tar -xzf - --strip-components=1 -C inputs/$platform
(cd inputs/$platform && sha256sum -c --quiet SHA256SUMS)
EOF
    done < <(jq -er '.platforms[]' "$work/run.json")
    cat <<EOF
cut -c67- signed.SHA256SUMS | while read -r f; do
    [ -e "signed/\$f" ] && continue
    for input in inputs/*/$name/artifacts/"\$f"; do [ -f "\$input" ] && { cp "\$input" signed/; break; }; done
done
(cd signed && sha256sum -c --quiet ../signed.SHA256SUMS)
[ "\$(ls signed | wc -l)" = "\$(wc -l < signed.SHA256SUMS)" ]
export ELASTOS_DATA_DIR=$RELEASE_SEED_DATA
publish() { $RELEASE_SEED_RUNTIME publish-release --version $version --channel canary --signed-publication signed \\
            --publisher-did $did --ipfs-provider-bin $RELEASE_SEED_DATA/bin/ipfs-provider "\$@"; }
sudo -v; [ -x $RELEASE_SEED_RUNTIME ]; [ -x $RELEASE_SEED_DATA/bin/ipfs-provider ]; [ -w $RELEASE_SEED_DATA ]
sudo systemctl stop $RELEASE_SEED_UNIT
trap 'sudo systemctl start $RELEASE_SEED_UNIT' EXIT  # the seed is never left down
publish --preflight-only
# Seeds on a Runtime older than the announcement removal still end with the old announcement error after commit; remove this once the seed runs a newer Runtime (#174).
publish 2>&1 | tee import.log || grep -q 'committed; retry publication to announce its head: No running runtime found' import.log
trap - EXIT; sudo systemctl start $RELEASE_SEED_UNIT
EOF
}

seed_upgrade() {
    [[ $# == 1 && "$1" =~ ^[1-9][0-9]*$ ]] || die "usage: seed-upgrade RUN_ID"
    need_env RELEASE_SEED_USER RELEASE_SEED_DATA RELEASE_SEED_UNIT RELEASE_SEED_STAGE RELEASE_SEED_RUNTIME
    local pkg id digest commit ci_run seed_proof_dir receipt source_tree artifact_attempt proof_id proof_digest proof_attempt
    pkg=$(checked_artifact "$1" release-seed)
    read -r id digest commit artifact_attempt <<< "$pkg"
    ci_run=$(checked_ci "$commit")
    echo "# Develop CI run: $(jq -r '[(.id | tostring), .url] | join(" ")' <<< "$ci_run")"
    mkdir -p "$WORK_ROOT"
    seed_proof_dir=$(mktemp -d "$WORK_ROOT/seed-verification.XXXXXXXX")
    trap 'rm -rf "$seed_proof_dir"' EXIT
    gh api "repos/$REPO/actions/artifacts/$id/zip" > "$seed_proof_dir/input.zip"
    [[ "$(sha "$seed_proof_dir/input.zip")" == "$digest" ]] || die "seed artifact digest differs"
    python3 -I -S "$ROOT/scripts/release-provenance-seal.py" unzip --input "$seed_proof_dir/input.zip" --output "$seed_proof_dir/package"
    read -r proof_id proof_digest _ proof_attempt <<< "$(checked_artifact "$1" release-proof-seed)"
    gh api "repos/$REPO/actions/artifacts/$proof_id/zip" > "$seed_proof_dir/proof.zip"
    [[ "$(sha "$seed_proof_dir/proof.zip")" == "$proof_digest" ]] || die "seed proof digest differs"
    python3 -I -S "$ROOT/scripts/release-provenance-seal.py" unzip --input "$seed_proof_dir/proof.zip" --output "$seed_proof_dir/proof"
    receipt=$(provenance seed --run "$1" --prefix release-seed --receipt "$seed_proof_dir/proof/seed-input.json" \
        --payload-root "$seed_proof_dir/package" --bundle "$seed_proof_dir/proof/provenance.jsonl") || die "attested seed receipt refused"
    source_tree=$(jq -er .source.tree <<< "$receipt")
    echo "# Approved source: $commit tree $source_tree; package run $1"
    seed_preamble "$RELEASE_SEED_STAGE/seed-upgrade-$1"
    cat <<EOF
sudo install -d -o $RELEASE_SEED_USER /opt/elastos
get seed $id $digest
$(jq -r '.files | to_entries[] | "echo " + ((.value.sha256 + "  seed/" + .key) | @sh) + " | sha256sum -c --quiet"' <<< "$receipt")
chmod 755 seed/elastos seed/ipfs-provider
rm -rf $SOURCE_ROOT.new; mkdir $SOURCE_ROOT.new; tar -xzf seed/source.tar.gz -C $SOURCE_ROOT.new
[ "\$(git -C $SOURCE_ROOT.new rev-parse HEAD)" = $commit ]
[ "\$(git -C $SOURCE_ROOT.new rev-parse 'HEAD^{tree}')" = $source_tree ]
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
    rm -rf "$seed_proof_dir"
    trap - EXIT
}

command="${1:-}"
case "$command" in
    prepare|policy|seed) shift; "$command" "$@" ;;
    seed-upgrade) shift; seed_upgrade "$@" ;;
    *) sed -n '2,25p' "${BASH_SOURCE[0]}"; exit 2 ;;
esac
