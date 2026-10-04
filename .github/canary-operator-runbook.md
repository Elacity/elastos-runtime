# Operator M1/M2 canary procedure

This package is for the operator-approved first canary hop in issue #89.
The operator owns key conversion, signing, seed deployment and Mac execution.
The CI builder supplies public inputs and source receipts. The test uses an
isolated Mac HOME; the existing Mac account and Home data stay in place.

## Approved input identities

- Repository: `Elacity/elastos-runtime`.
- Exact source commit and tree: read `receipts/build.json` and match them to
  the accepted source and CI receipt linked in #89.
- Source CI: <https://github.com/Elacity/elastos-runtime/actions/runs/37086607684>.
- Native builder: <https://github.com/Elacity/elastos-runtime/actions/runs/37088374176>.
- Draft versions: V1 `0.8.0-alpha.1`, V2 `0.8.0-alpha.2`; channel `canary`.
- Origin: `https://elastos.elacitylabs.com`.
- Public signer: `did:key:z6MkgwHd2BCWe1jHMXPiR6H1q1RFPcv1YzhMbK5G1kBarbfe`.
- The public holder node/ticket are retained in `public-baseline/carrier-bootstrap.json`.
  The holder's transport DID is separate from the release signer.
- Reviewed signer SHA-256: `e8904cfb98a076544264117dbc92e7fbeae11f9cf9173fa07e1782595381d5c2`.
- Mac Python: load `python.path` from the verified policy draft;
  SHA-256 `fe46716a94d8efa4514feb3c39ba3e270deee2187556986f6ddcff54aba7bb9a`.
- Mac OpenSSL: load `openssl.path` from the verified policy draft;
  SHA-256 `5d8f84484b7317ec5639ce68ccecc1d6f565ca6df483c8ae731e25265d83466d`.

The policy drafts contain the exact unsigned-manifest hashes and develop pin.
Anders approves the versions, source/tool pins, custody paths and seed inputs
before signing. A changed develop head requires renewed policy approval.
Stable signing retains the version-tag-on-main gate.

## 1. Download and verify

Use one dedicated Bash shell for the Mac commands below, and a separate Bash
shell on the seed. Each block stops if a check fails.

Use the completed **handoff packaging run and artifact name linked in #89**.
Set `HANDOFF_RUN` and `HANDOFF_ARTIFACT` to those exact values. Download to a
new stable directory with at least 15% free disk after extraction:

```bash
set -euo pipefail
umask 077
HANDOFF_RUN='COPY_THE_COMPLETED_PACKAGING_RUN_ID_FROM_89'
HANDOFF_ARTIFACT='COPY_THE_EXACT_PACKAGED_ARTIFACT_NAME_FROM_89'
EXPECTED_TAR_SHA256='COPY_THE_HANDOFF_TAR_SHA256_FROM_89'
TRANSFER="$HOME/.local/share/elastos-canary-transfer"
test ! -e "$TRANSFER"
mkdir -p "$TRANSFER"
gh run download "$HANDOFF_RUN" --repo Elacity/elastos-runtime \
  --name "$HANDOFF_ARTIFACT" --dir "$TRANSFER"
(cd "$TRANSFER" && shasum -a 256 -c SHA256SUMS)
test "$(shasum -a 256 "$TRANSFER/canary-handoff.tar.gz" | cut -d ' ' -f 1)" = "$EXPECTED_TAR_SHA256"
mkdir "$TRANSFER/inputs"
tar -xzf "$TRANSFER/canary-handoff.tar.gz" -C "$TRANSFER/inputs" --strip-components=1
INPUTS="$TRANSFER/inputs"
(cd "$INPUTS" && shasum -a 256 -c SHA256SUMS)
shasum -a 256 "$INPUTS/unsigned-V1/signing-input.json" \
  "$INPUTS/unsigned-V2-PROVISIONAL/signing-input.json"
```

Read `receipts/build.json`, `receipts/public-verification.json`,
`receipts/executable-versions.json`, `receipts/process-cleanup.json` and the
packaging receipt. Match the artifact ID/digest and successful run in GitHub,
the source identities above, exact executable versions, and empty owned-process
cleanup. The packaging receipt verifies the original ZIP digest before it
restores native executable modes from checksum-bound platform receipts.

V1 is ready for custody review. **V2-PROVISIONAL is retained native preparation,
not a signing candidate.** Its policy has a null manifest hash. Actual signed
V1 envelope CIDs are required to finalize V2's chain. Keep N1/N2, the original
`support-input.json`/`support_origin`, native CID maps, stamps and frozen installer.

## 2. Qualify the seed receipt and custody policy

The operator compares `public-baseline/suggested-publish-state.json` with the
existing seed Runtime's actual public Publisher receipt: compare the public
signer, version, head CID and release CID. If these fields differ, ask the
key-free builder to re-prepare V1 against the actual receipt. Retain the seed's
actual publication timestamp, which can record import time rather than the
signed head time. The suggested CI receipt is comparison evidence; the seed
keeps its actual receipt and holder identity. For a verified absent legacy
receipt, step 5 reconstructs the observed public chain under Anders's approval.
Canonical Runtime derives its Publisher state path from its selected data root;
`ELASTOS_PUBLISH_STATE_DIR` alone does not select that root.

Create a protected custody directory **outside INPUTS**. Install only the
reviewed signer there. Replace the policy's `/OPERATOR/` paths with canonical
absolute paths to the installed signer and operator-held PEM; keep all ancestor
directories protected from group/other writes. The custodian account owns the
single-link key file with mode 0600. Python/OpenSSL pins above were qualified for
Anders's Mac account; a different custodian account qualifies ownership anew.

```bash
set -euo pipefail
CUSTODY="$HOME/.local/share/elastos-canary-custody"
test ! -e "$CUSTODY"
mkdir -p "$CUSTODY"
chmod 700 "$CUSTODY"
install -m 600 "$INPUTS/release-signer.py" "$CUSTODY/release-signer.py"
cp "$INPUTS/policy-drafts/unsigned-V1.json" "$CUSTODY/V1-policy.json"
chmod 600 "$CUSTODY/V1-policy.json"
PINNED_PYTHON=$(python3 -I -S -c 'import json,sys; print(json.load(open(sys.argv[1]))["python"]["path"])' "$INPUTS/policy-drafts/unsigned-V1.json")
PINNED_OPENSSL=$(python3 -I -S -c 'import json,sys; print(json.load(open(sys.argv[1]))["openssl"]["path"])' "$INPUTS/policy-drafts/unsigned-V1.json")
INSTALLED_SIGNER="$CUSTODY/release-signer.py"
PEM_KEY="$CUSTODY/maintainer-ed25519.pem"
test "$(shasum -a 256 "$INSTALLED_SIGNER" | cut -d ' ' -f 1)" = 'e8904cfb98a076544264117dbc92e7fbeae11f9cf9173fa07e1782595381d5c2'
test "$(shasum -a 256 "$PINNED_PYTHON" | cut -d ' ' -f 1)" = 'fe46716a94d8efa4514feb3c39ba3e270deee2187556986f6ddcff54aba7bb9a'
test "$(shasum -a 256 "$PINNED_OPENSSL" | cut -d ' ' -f 1)" = '5d8f84484b7317ec5639ce68ccecc1d6f565ca6df483c8ae731e25265d83466d'
"$PINNED_PYTHON" --version
"$PINNED_OPENSSL" version
```

The operator edits `V1-policy.json`: `tool.path` becomes `INSTALLED_SIGNER`
and `key_path` becomes `PEM_KEY`. Use the following public checks for either
V1 or V2, with `POLICY` set to that version's custody policy:

```bash
set -euo pipefail
POLICY="$CUSTODY/V1-policy.json"
SOURCE_COMMIT=$("$PINNED_PYTHON" -I -S -c 'import json,sys; print(json.load(open(sys.argv[1]))["commit"])' "$POLICY")
CURRENT_DEVELOP=$(gh api repos/Elacity/elastos-runtime/git/ref/heads/develop --jq .object.sha)
gh api "repos/Elacity/elastos-runtime/compare/$SOURCE_COMMIT...$CURRENT_DEVELOP?per_page=1" \
  --jq '{status, behind_by, base: .base_commit.sha, merge_base: .merge_base_commit.sha}'
```

The comparison must report `ahead` or `identical`, `behind_by = 0`, and the
frozen source commit for both `base` and `merge_base`. If develop has moved,
Anders approves `CURRENT_DEVELOP` as the replacement `develop_oid` before
the operator edits that field. Keep the source commit/tree, manifest hash,
native bytes, tool pins and quotas from the reviewed draft. Anders then
approves the exact policy file hash, manifest hash, custody paths and version.
Compute the edited policy hash with `shasum -a 256 "$POLICY"`.
Record the public approval and hashes in #89; custody paths stay private.
Immediately before signing, compare the policy's `develop_oid` with a fresh
`gh api repos/Elacity/elastos-runtime/git/ref/heads/develop --jq .object.sha`.
A difference returns to this approval step; the signer also checks the pin.

## 3. Operator-only hex-to-PEM conversion

Set `HEX_KEY` to the operator's existing protected **32-byte Ed25519 seed stored
as 64 hex characters**. The key stays on this Mac under custody. The following
operator command reads it directly, confirms the public DID and creates one
exclusive mode-0600 PEM. It passes secret bytes through OpenSSL's stdin, with
no secret command argument, environment value, output or intermediate key file.
The DER form is PKCS#8 Ed25519 (RFC 8410).

```bash
set -euo pipefail
HEX_KEY='/OPERATOR/EXISTING_PROTECTED_HEX_KEY'
env -i "$PINNED_PYTHON" -I -S - "$HEX_KEY" "$PEM_KEY" "$PINNED_OPENSSL" <<'PY'
import base64, os, re, stat, subprocess, sys
from pathlib import Path
source, target, openssl = map(Path, sys.argv[1:])
for path in (source, target):
    assert path.is_absolute() and path == path.resolve()
    for parent in path.parents:
        info = parent.lstat()
        assert stat.S_ISDIR(info.st_mode) and info.st_uid in (0, os.getuid()) and not info.st_mode & 0o022
fd = os.open(source, os.O_RDONLY | os.O_NOFOLLOW)
try:
    info = os.fstat(fd)
    assert stat.S_ISREG(info.st_mode) and info.st_uid == os.getuid() and info.st_nlink == 1 and not info.st_mode & 0o077
    text = os.read(fd, 1024).strip()
    assert re.fullmatch(rb'[0-9a-fA-F]{64}', text)
finally:
    os.close(fd)
der = bytes.fromhex('302e020100300506032b657004220420') + bytes.fromhex(text.decode('ascii'))
reply = subprocess.run([str(openssl), 'pkey', '-provider', 'default', '-inform', 'DER', '-pubout', '-outform', 'DER'],
                       input=der, capture_output=True, timeout=20, env={'OPENSSL_CONF': '/dev/null', 'LANG': 'C'})
assert reply.returncode == 0 and len(reply.stdout) == 44 and reply.stdout[:12] == bytes.fromhex('302a300506032b6570032100')
alphabet = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz'
number = int.from_bytes(bytes.fromhex('ed01') + reply.stdout[12:], 'big')
encoded = ''
while number:
    number, digit = divmod(number, 58)
    encoded = alphabet[digit] + encoded
did = 'did:key:z' + encoded
assert did == 'did:key:z6MkgwHd2BCWe1jHMXPiR6H1q1RFPcv1YzhMbK5G1kBarbfe'
body = base64.b64encode(der)
pem = b'-----BEGIN PRIVATE KEY-----\n' + b'\n'.join(body[i:i+64] for i in range(0, len(body), 64)) + b'\n-----END PRIVATE KEY-----\n'
fd = os.open(target, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
try:
    with os.fdopen(fd, 'wb') as output:
        output.write(pem)
        output.flush()
        os.fsync(output.fileno())
except BaseException:
    target.unlink()
    raise
print('Protected PEM created for the approved public DID.')
PY
```

## 4. Sign V1 and copy public bytes

Run the installed signer with an empty inherited environment and the approved
policy. Confirm the **complete public DID** at its prompt. Keep the output path
absent before this command. Signing performs public source, file, DID and
signature checks before retaining its read-only set.

```bash
set -euo pipefail
SIGNED_V1="$CUSTODY/signed-V1"
env -i "$PINNED_PYTHON" -I -S "$INSTALLED_SIGNER" \
  --policy "$CUSTODY/V1-policy.json" \
  --input-root "$INPUTS/unsigned-V1" --output-root "$SIGNED_V1"
cmp "$SIGNED_V1/install.sh" "$INPUTS/frozen-install.sh"
(cd "$SIGNED_V1" && shasum -a 256 *) > "$CUSTODY/signed-V1-SHA256SUMS"
```

Use the operator's existing private SSH alias and a new stable seed staging
directory, represented below by `SEED_ALIAS` and `SEED_STAGE`. Copy the **signed
public set and checksum receipt only**; the custody key and policy stay on Mac.

```bash
set -euo pipefail
SEED_ALIAS='OPERATOR_EXISTING_SSH_ALIAS'
SEED_STAGE='/OPERATOR/OWNED_STABLE_CANARY_STAGE'
scp -r "$SIGNED_V1" "$CUSTODY/signed-V1-SHA256SUMS" "$SEED_ALIAS:$SEED_STAGE/"
```

## 5. Qualify or upgrade the seed publisher

Use the Linux x86_64 Runtime handoff linked in #89, built from the same source
at the **operator-selected stable helper root**. Verify its archive/checksums,
build receipt, source tree, Runtime version/hash and recorded embedded helper
root. Install the reviewed source helpers at that exact root. The existing
seed provider's qualified path and hash remain explicit publication inputs.

On the Mac, download the exact Linux run and artifact named in the handoff.
Use a new transfer directory and the approved outer tar hash from #89:

```bash
set -euo pipefail
LINUX_TRANSFER="$CUSTODY/linux-public-transfer"
test ! -e "$LINUX_TRANSFER"
mkdir -m 700 "$LINUX_TRANSFER"
gh run download "$LINUX_RUN" --repo Elacity/elastos-runtime \
  --name "$LINUX_ARTIFACT" --dir "$LINUX_TRANSFER"
test "$(shasum -a 256 "$LINUX_TRANSFER/seed-publisher.tar.gz" | cut -d ' ' -f 1)" = "$APPROVED_LINUX_TAR_SHA256"
mkdir -m 700 "$LINUX_TRANSFER/verified"
tar -xzf "$LINUX_TRANSFER/seed-publisher.tar.gz" -C "$LINUX_TRANSFER/verified"
cd "$LINUX_TRANSFER/verified"
shasum -a 256 -c SHA256SUMS
```

The Linux receipt records its exact helper root, Runtime/provider hashes,
Rust toolchain, source parity and Ubuntu/glibc ABI. The archive qualifies
signed import/export; unsigned preparation also needs the frozen installer's
Git objects. Transfer only these verified public files to the seed. Extract
`source.tar.gz` at the receipt's helper root as the existing service owner,
preserving `elastos/crates/elastos-server` and the complete reviewed source
tree. Install the two binaries at operator-selected stable paths with mode
0700, compare their hashes with the receipt, and pass the qualified provider
explicitly. Its Kubo qualification and the real import remain seed checks.

The copied Runtime needs `scripts/publish-release.sh` and its reviewed Python
helper at the embedded root even when it imports an already signed set.
`--dry-run` returns before that helper check; use `--preflight-only` as well.
If the existing seed Runtime differs from the reviewed candidate, the operator
first completes the baseline and receipt steps below during the approved
window, then updates the intentional Runtime/helper files. Before the approved
restart, keep the existing gateway config and set
`gateway_allowed_hosts = ["elastos.elacitylabs.com"]` and
`gateway_public_publisher_bootstrap = true` for the approved public origin and
holder. Retain the previous config with the approved rollback. Restart the
existing instance with its retained data root, identity, holder, provider
configuration and service arguments. Verify binary parity after restart and
the public host/bootstrap after import. Use the existing publication instance;
its one current signed set is replaced at the first approved new-key canary
import.

The legacy instance reported in #89 has no Publisher receipt. Its old served
release also lacks the installer hash required by the new gateway admission.
Anders approves an upgrade/import window that preserves the old files and
their recovery path while the new Runtime imports the first valid canary set.
Successful source or baseline checks alone do not prove old-set serving after
the Runtime replacement. Verify the approved new served bytes after import.

For V2, retain the actual committed V1 receipt and skip the old-baseline
receipt reconstruction and exclusive migration below. These steps apply
only to the approved first V1 import with a verified absent legacy receipt.
Before that first import, reconstruct the absent receipt from the verified
old public baseline under Anders's approval. An empty receipt would refuse
V1's predecessor CIDs even with `--allow-signer-rotation`. Keep the old DID
and CIDs in this migration; the signed import owns the change to the new DID.

On the Mac, use the pinned public-data tools to verify that the origin still
serves the exact baseline bytes accepted in steps 1–2, recompute its CIDs, and
bind them to V1. This writes a public receipt only:

```bash
set -euo pipefail
"$PINNED_PYTHON" -I -S - "$INPUTS" "$CUSTODY/verified-baseline-receipt.json" <<'PY'
import hashlib, importlib.util, json, sys, urllib.request
from pathlib import Path
inputs, destination = map(Path, sys.argv[1:])
tool = inputs / 'release-signer.py'
assert hashlib.sha256(tool.read_bytes()).hexdigest() == 'e8904cfb98a076544264117dbc92e7fbeae11f9cf9173fa07e1782595381d5c2'
spec = importlib.util.spec_from_file_location('public_signer', tool)
signer = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = signer
spec.loader.exec_module(signer)
class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args):
        raise ValueError('Baseline redirect refused')
opener = urllib.request.build_opener(NoRedirect())
envelopes, raw = {}, {}
for name in ['release-head.json', 'release.json']:
    with opener.open('https://elastos.elacitylabs.com/' + name, timeout=30) as reply:
        data = reply.read(256 * 1024 + 1)
    assert len(data) <= 256 * 1024
    assert data == (inputs / 'public-baseline' / name).read_bytes(), 'Public baseline changed; renew builder approval'
    envelope = signer.parse_json(data)
    assert envelope['signer_did'] == 'did:key:z6MkrFPDgDi98Ek6AFHM3VT9bVJytnDf5mfHAV6gyrD5frYj'
    envelopes[name], raw[name] = envelope, data
head, release = (envelopes[name]['payload'] for name in ['release-head.json', 'release.json'])
assert head['version'] == release['version'] == '0.7.1'
assert head['channel'] == release['channel'] == 'stable'
assert head['release_sha256'] == signer.sha256(raw['release.json'])
release_cid = signer.unixfs_metadata_cid(raw['release.json'])
head_cid = signer.unixfs_metadata_cid(raw['release-head.json'])
assert head['latest_release_cid'] == release_cid
v1 = signer.parse_json((inputs / 'unsigned-V1/signing-input.json').read_bytes())
assert v1['head']['prev_head_cid'] == head_cid
assert v1['release']['prev_release_cid'] == release_cid
receipt = signer.parse_json((inputs / 'public-baseline/suggested-publish-state.json').read_bytes())
assert receipt == {'publisher_did': envelopes['release-head.json']['signer_did'],
                   'last_release_cid': release_cid, 'last_head_cid': head_cid,
                   'last_version': '0.7.1', 'last_published_at': head['updated_at']}
with destination.open('x') as output:
    output.write(json.dumps(receipt, indent=2) + '\n')
destination.chmod(0o600)
print('Verified public baseline receipt:', signer.sha256(destination.read_bytes()))
PY
```

The retained baseline's signatures were independently verified in CI; exact
byte equality preserves that verification. Record that `last_published_at`
is reconstructed from the signed head because the old import time is unknown.
Copy this public receipt to a verified staging path on the seed and compare
its SHA-256. As the Runtime service owner, set `SEED_DATA_DIR` to the existing
instance's selected data directory and `BASELINE_RECEIPT` to that verified
staging file. The canonical destination belongs to Publisher; consumer trust
configuration stays with the installed client.

During the approved window, stop the existing Runtime before changing its
Publisher parents. If the three existing directories `ElastOS`,
`SystemServices` and `Publisher` have legacy group write, run this step as
the service owner. It checks all three before changing permissions, removes
only group write and preserves their other mode bits. The absolute data root
keeps its permissions. Links, foreign owners and world write require a new
operator decision; this step leaves their permissions intact.

```bash
set -euo pipefail
python3 -I -S - "$SEED_DATA_DIR" <<'PY'
import os, stat, sys
from pathlib import Path
data = Path(sys.argv[1])
assert data.is_absolute() and '..' not in data.parts
flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW
descriptors = [os.open('/', flags)]
try:
    for part in data.parts[1:]:
        descriptors.append(os.open(part, flags, dir_fd=descriptors[-1]))
    metadata = os.fstat(descriptors[-1])
    assert stat.S_ISDIR(metadata.st_mode) and metadata.st_uid == os.geteuid() and metadata.st_mode & 0o022 == 0
    approved = []
    for name in ['ElastOS', 'SystemServices', 'Publisher']:
        descriptor = os.open(name, flags, dir_fd=descriptors[-1])
        descriptors.append(descriptor)
        metadata = os.fstat(descriptor)
        assert stat.S_ISDIR(metadata.st_mode) and metadata.st_uid == os.geteuid() and metadata.st_mode & 0o002 == 0
        approved.append((descriptor, metadata.st_dev, metadata.st_ino, stat.S_IMODE(metadata.st_mode)))
    for descriptor, device, inode, mode in approved:
        os.fchmod(descriptor, mode & ~stat.S_IWGRP)
        metadata = os.fstat(descriptor)
        assert (metadata.st_dev, metadata.st_ino, metadata.st_uid) == (device, inode, os.geteuid())
        assert stat.S_IMODE(metadata.st_mode) == mode & ~stat.S_IWGRP
    print('Publisher parent group write removed; other permissions and files preserved.')
finally:
    for descriptor in reversed(descriptors):
        os.close(descriptor)
PY
```

Run the strict receipt migration below after this permission check. A writable
data root remains a refused case and requires an operator decision.

```bash
set -euo pipefail
python3 -I -S - "$SEED_DATA_DIR" "$BASELINE_RECEIPT" <<'PY'
import json, os, stat, sys
from pathlib import Path
data, source = map(Path, sys.argv[1:])
data = data.resolve(strict=True)
assert data.is_dir()
raw = source.read_bytes()
assert len(raw) <= 65536
receipt = json.loads(raw)
assert set(receipt) == {'publisher_did', 'last_release_cid', 'last_head_cid', 'last_version', 'last_published_at'}
assert receipt['publisher_did'] == 'did:key:z6MkrFPDgDi98Ek6AFHM3VT9bVJytnDf5mfHAV6gyrD5frYj'
assert receipt['last_version'] == '0.7.1'
root = data
for part in ['ElastOS', 'SystemServices', 'Publisher']:
    metadata = root.lstat()
    assert stat.S_ISDIR(metadata.st_mode) and metadata.st_uid == os.geteuid() and metadata.st_mode & 0o022 == 0
    root = root / part
    try:
        root.mkdir(mode=0o700)
    except FileExistsError:
        pass
metadata = root.lstat()
assert stat.S_ISDIR(metadata.st_mode) and metadata.st_uid == os.geteuid() and metadata.st_mode & 0o022 == 0
directory = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
try:
    descriptor = os.open('publish-state.json', os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600, dir_fd=directory)
    with os.fdopen(descriptor, 'wb') as output:
        output.write(raw)
        output.flush()
        os.fsync(output.fileno())
        metadata = os.fstat(output.fileno())
        assert metadata.st_uid == os.geteuid() and metadata.st_nlink == 1 and stat.S_IMODE(metadata.st_mode) == 0o600
    os.fsync(directory)
finally:
    os.close(directory)
print('Installed the approved old-baseline Publisher receipt; existing receipts are preserved.')
PY
```

This exclusive migration refuses an existing receipt. Compare any receipt that
appears with the approved baseline before proceeding. Runtime selects Linux
data from `XDG_DATA_HOME/elastos`, otherwise `HOME/.local/share/elastos`; use
the existing service environment for migration and all publication commands.

On the seed, set `SEED_RUNTIME`, `SIGNED_SET`, and `IPFS_PROVIDER` to their
verified absolute paths. Run in the existing instance's approved HOME/data-root
environment. For V1 and V2 alike, use a protected umask for the publication
snapshot:

```bash
set -euo pipefail
umask 077
V1='0.8.0-alpha.1'
DID='did:key:z6MkgwHd2BCWe1jHMXPiR6H1q1RFPcv1YzhMbK5G1kBarbfe'
"$SEED_RUNTIME" publish-release --version "$V1" --channel canary \
  --signed-publication "$SIGNED_SET" --publisher-did "$DID" \
  --dry-run --allow-signer-rotation
"$SEED_RUNTIME" publish-release --version "$V1" --channel canary \
  --signed-publication "$SIGNED_SET" --publisher-did "$DID" \
  --ipfs-provider-bin "$IPFS_PROVIDER" --preflight-only --allow-signer-rotation
if "$SEED_RUNTIME" publish-release --version "$V1" --channel canary \
  --signed-publication "$SIGNED_SET" --publisher-did "$DID" \
  --ipfs-provider-bin "$IPFS_PROVIDER" --allow-signer-rotation; then
  IMPORT_STATUS=0
else
  IMPORT_STATUS=$?
fi
printf 'Import exit status: %s\n' "$IMPORT_STATUS"
```

The first import asks for the complete DID to change the saved public pin.
Retain the terminal output and read it before proceeding. The frozen Runtime
can exit with this exact postcommit gossip error on the gateway publication
instance:

```text
Error: The signed set is committed; retry publication to announce its head: No running runtime found. Start `elastos serve` first for gossip announcements.
```

This error occurs after Publisher commits the signed set, receipt and head.
The gateway owns a separate peer control endpoint; the frozen announcement
code selects serve coordinates and its gossip calls require the serve API.
Keep the gateway's existing control configuration. Direct Carrier pulls read
the committed Publisher files and receipt. Gossip repair remains an #186 gate.

Under the approved M1/M2 procedure, the operator can record this exact error
as an outstanding gossip result and continue only after the checks below.
Any other unexpected output or failed check stops the procedure for review.
Keep the full output and exit status; receipt parity and Carrier proof establish
this hop's result. Record gossip delivery only when it has separate evidence.

As the service owner, retain the actual
`$SEED_DATA_DIR/ElastOS/SystemServices/Publisher/publish-state.json`. Compare
the committed `release-head.json`, `release.json` and `install.sh` in that
directory byte for byte with `SIGNED_SET`, and record their SHA-256 hashes.
Verify the receipt's signer and version, recompute both signed-envelope CIDs
with the reviewed public-data signer, and match them to `last_head_cid` and
`last_release_cid`. Keep the actual `last_published_at` from the committed
receipt. Verify public envelope/installer hashes against the same set and
the public bootstrap against the approved holder. The plain Carrier check
in step 6 must pass before V2 signing; step 8's check must pass before M2
applies V2. A failed import uses the helper's recovery result; preserve that
evidence and the prior chain while the operator resolves it.

Run the committed-file checks on the seed for each version, then copy only
its public receipt back to the Mac custody directory:

```bash
set -euo pipefail
PUBLISHER="$SEED_DATA_DIR/ElastOS/SystemServices/Publisher"
for name in release-head.json release.json install.sh; do
  cmp "$PUBLISHER/$name" "$SIGNED_SET/$name"
  sha256sum "$PUBLISHER/$name" "$SIGNED_SET/$name"
done
sha256sum "$PUBLISHER/publish-state.json"
```

Set `SEED_RECEIPT` on the Mac to this seed receipt's absolute path. Set
`PUBLIC_RECEIPT` to a new custody path, `PUBLIC_SET` to the corresponding
`SIGNED_V1` or `SIGNED_V2`, and `PUBLIC_VERSION` to its approved version.
After the public-only copy, match its SHA-256 to the seed result and verify
the CIDs with the reviewed signer already installed on the Mac:

```bash
set -euo pipefail
scp "$SEED_ALIAS:$SEED_RECEIPT" "$PUBLIC_RECEIPT"
shasum -a 256 "$PUBLIC_RECEIPT"
"$PINNED_PYTHON" -I -S - "$INSTALLED_SIGNER" "$PUBLIC_SET" "$PUBLIC_RECEIPT" "$PUBLIC_VERSION" <<'PY'
import hashlib, importlib.util, sys
from pathlib import Path
tool, public, receipt_path = map(Path, sys.argv[1:4])
version = sys.argv[4]
assert hashlib.sha256(tool.read_bytes()).hexdigest() == 'e8904cfb98a076544264117dbc92e7fbeae11f9cf9173fa07e1782595381d5c2'
spec = importlib.util.spec_from_file_location('public_signer', tool)
signer = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = signer
spec.loader.exec_module(signer)
head_bytes = (public / 'release-head.json').read_bytes()
release_bytes = (public / 'release.json').read_bytes()
head_envelope, release_envelope = map(signer.parse_json, (head_bytes, release_bytes))
head, release = head_envelope['payload'], release_envelope['payload']
receipt = signer.parse_json(receipt_path.read_bytes())
did = 'did:key:z6MkgwHd2BCWe1jHMXPiR6H1q1RFPcv1YzhMbK5G1kBarbfe'
assert head_envelope['signer_did'] == release_envelope['signer_did'] == receipt['publisher_did'] == did
assert head['version'] == release['version'] == receipt['last_version'] == version
assert head['channel'] == release['channel'] == 'canary'
assert head['release_sha256'] == signer.sha256(release_bytes)
assert head['latest_release_cid'] == receipt['last_release_cid'] == signer.unixfs_metadata_cid(release_bytes)
assert receipt['last_head_cid'] == signer.unixfs_metadata_cid(head_bytes)
assert release['installer_sha256'] == signer.sha256((public / 'install.sh').read_bytes())
assert type(receipt['last_published_at']) is int and receipt['last_published_at'] >= 0
print('Committed public receipt matches signed bytes:', receipt['last_head_cid'], receipt['last_release_cid'])
PY
```

## 6. M1 on the isolated Mac HOME

Keep one stable test HOME across both steps. Use the frozen installer already
verified against **signed V1**. This is the accepted CLI-only first hop;
normal Home acceptance still has the signed capsule-delivery gate in #217.

```bash
set -euo pipefail
TEST_HOME="$HOME/.local/share/elastos-real-key-m1m2/home"
test ! -e "$TEST_HOME"
mkdir -p "$TEST_HOME"
TEST_PATH='/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin'
env -i HOME="$TEST_HOME" PATH="$TEST_PATH" \
  bash "$SIGNED_V1/install.sh" --install-only
MAC_RUNTIME="$TEST_HOME/.local/bin/elastos"
env -i HOME="$TEST_HOME" PATH="$TEST_PATH" "$MAC_RUNTIME" --version \
  > "$CUSTODY/M1-version.stdout" 2> "$CUSTODY/M1-version.stderr"
test "$(cat "$CUSTODY/M1-version.stdout")" = 'elastos 0.8.0-alpha.1'
test ! -s "$CUSTODY/M1-version.stderr"
"$PINNED_PYTHON" -I -S - "$SIGNED_V1/release.json" "$MAC_RUNTIME" <<'PY'
import hashlib, json, sys
from pathlib import Path
release = json.loads(Path(sys.argv[1]).read_bytes())['payload']
assert release['version'] == '0.8.0-alpha.1' and release['channel'] == 'canary'
expected = release['platforms']['aarch64-darwin']['binary']['sha256']
assert hashlib.sha256(Path(sys.argv[2]).read_bytes()).hexdigest() == expected
print('M1 installed binary matches the signed release hash.')
PY
env -i HOME="$TEST_HOME" PATH="$TEST_PATH" "$MAC_RUNTIME" update --check
```

Record the selected source, config/data/support fingerprints and exact signed
receipt before M2. The plain Carrier check must reach committed V1 and report
the current installation as up to date. Retain its transport evidence and
confirm zero HTTP fallback before V2 signing. Run the agreed disposable-key
tampered-binary and wrong-signer
refusal fixture in its separate test HOME, with the frozen trust pin, preserving
the positive real-key HOME. Production signing admits valid sets only. The
complete positive/refusal CI rehearsal is linked in #89; operator receipts are
separate acceptance evidence.

## 7. Finalize, sign and publish V2

After V1 is public, the key-free builder verifies its signed public envelopes
and committed predecessor CIDs, then
produces a metadata-only V2 finalization artifact from retained N2. It keeps
the native bytes, original support input/origin, installer stamps and frozen
installer. The builder uses public data; the operator keeps the real key.
The artifact carries `receipts/V2-finalization.json`, the final
`unsigned-V2/signing-input.json` and `policy-drafts/unsigned-V2.json`.
Compare `public-V1/suggested-publish-state.json` with the actual committed
seed V1 receipt before signing. The signer, version and head/release CIDs
must agree; retain the actual receipt's import timestamp.

Verify `public-V1/carrier-bootstrap.json` and
`receipts/bootstrap-routing-verification.json`. The routing receipt permits
only an optional IPv6 port change in the current public ticket and records
the two ticket hashes/ports. All other decoded ticket fields and the holder
identity stay fixed. Use its recorded refusal qualification when reviewing
this allowance. Anders approves the final manifest hash and the exact
source/develop/tool pins. V2's final policy replaces the provisional policy.

Download and verify that completed artifact as in step 1, using its own
completed run, artifact name and approved tar hash from #89. Set `V2_INPUTS`
to its final `unsigned-V2` root. Copy `policy-drafts/unsigned-V2.json` to
`$CUSTODY/V2-policy.json` with mode 0600 and set the same custody tool/key paths.
Run step 2's develop-pin comparison and exact policy approval for V2 before
signing. Then:

```bash
set -euo pipefail
SIGNED_V2="$CUSTODY/signed-V2"
env -i "$PINNED_PYTHON" -I -S "$INSTALLED_SIGNER" \
  --policy "$CUSTODY/V2-policy.json" \
  --input-root "$V2_INPUTS" --output-root "$SIGNED_V2"
cmp "$SIGNED_V2/install.sh" "$SIGNED_V1/install.sh"
(cd "$SIGNED_V2" && shasum -a 256 *) > "$CUSTODY/signed-V2-SHA256SUMS"
scp -r "$SIGNED_V2" "$CUSTODY/signed-V2-SHA256SUMS" "$SEED_ALIAS:$SEED_STAGE/"
```

Repeat seed dry-run, preflight and import from step 5 with V2 version
`0.8.0-alpha.2` and its verified signed-set path. Retain the V2 output and the
actual committed V1 receipt as the predecessor. Skip the first-receipt
migration. Retain the committed V2 receipt and repeat step 5's parity checks
and exact gossip-error rule before step 8.

## 8. Plain Carrier M2, preservation and repeat

Use the same Mac test HOME and frozen installation. These plain commands select
its saved Carrier source; retain their output and transport evidence:

```bash
set -euo pipefail
env -i HOME="$TEST_HOME" PATH="$TEST_PATH" "$MAC_RUNTIME" update --check
env -i HOME="$TEST_HOME" PATH="$TEST_PATH" "$MAC_RUNTIME" update
env -i HOME="$TEST_HOME" PATH="$TEST_PATH" "$MAC_RUNTIME" --version
env -i HOME="$TEST_HOME" PATH="$TEST_PATH" "$MAC_RUNTIME" update
```

Check/apply reaches `elastos 0.8.0-alpha.2`; the second apply reports up to date.
Verify the installed binary against signed V2 using step 6's hash command with
V2's version, and compare config/data/support, capsules and catalogue to the
M1 baseline. The approved source's installed-version/head fields advance;
its channel, signer, holder node, ticket and gateway remain fixed. Record clean
user output, Carrier delivery and zero HTTP fallback, plus separate tampered
and wrong-signer refusal evidence. Stop only test-owned processes when done.
Keep the test HOME and its receipts under the agreed retention condition.

Post the public versions, CI/artifact links, signed hash/CID receipts, operator
M1/M2 results and any remaining acceptance gap in #89. Broader promotion keeps
the recovery, #213, installed Home and Anders approval gates.

## Holder provider installation after qualification

The holder keeps its accepted Runtime and frozen helper tree. The repaired
`ipfs-provider` has its own reviewed source identity. Record both identities in
#89. The Canary Linux holder workflow accepts an exact provider commit and tree
for proof. Select `package_merged=true` only after that commit is merged into
`develop`, the required checks pass, and the independent review passes. The
workflow also checks merge ancestry before it creates the operator package.

Download the completed `qualified-holder-linux-<provider-commit>-<attempt>`
artifact from the run recorded in #89. Verify its approved archive hash, then
verify `qualified-holder/SHA256SUMS`. Its `holder-idle.json` must report
`passed=true`, completed cleanup, the accepted Runtime identity, and the exact
merged provider identity. Its gateway and Carrier reads use uncached content
after real production watcher stops. The fixture records its accelerated
`last_used` input. The separate bounded proof uses a real native provider;
Runtime tests own the bounded Carrier prepare/retry proof.

Take the exact run, artifact name, provider source, archive digest and component
pin from the current #89 operator approval. Set `HOLDER_RUN`,
`HOLDER_ARTIFACT_NAME`, `HOLDER_PROVIDER_COMMIT`, and
`HOLDER_ARCHIVE_SHA256` to those approved public inputs. Check the event and
branch independently before accepting its receipts:

```bash
gh api "repos/Elacity/elastos-runtime/actions/runs/$HOLDER_RUN" \
  --jq '{event,head_branch,head_sha,path,status,conclusion}'
```

Require `workflow_dispatch`, `develop`, the approved `HOLDER_PROVIDER_COMMIT`,
`.github/workflows/canary-holder-linux.yml`, `completed`, and `success`.
Download into a new private directory. On the Mac use `shasum -a 256` in place
of `sha256sum`:

```bash
umask 077
mkdir "$HOLDER_DOWNLOAD"
gh run download "$HOLDER_RUN" --repo Elacity/elastos-runtime \
  --name "$HOLDER_ARTIFACT_NAME" \
  --dir "$HOLDER_DOWNLOAD"
(cd "$HOLDER_DOWNLOAD" && sha256sum -c qualified-holder.tar.gz.sha256)
```

Match that tar hash to `HOLDER_ARCHIVE_SHA256` before extracting it. Its
members are regular files under `qualified-holder/`; refuse absolute paths,
parent traversal, symlinks and hardlinks. Copy only verified public package
files to the seed through the operator's established transfer path.

`provider-build.json` identifies the new binary. `accepted-build.json`
identifies the previously accepted Runtime and its original build; use its
`elastos` record for Runtime parity. `provider-source.tar.gz` contains the
repaired source. `runtime-helper-source.tar.gz` contains the frozen helper
source used by the accepted Runtime. The provider binary does not embed that
helper root. The installed Runtime continues to use its approved stable helper
root. This package changes only the provider.

Anders approves the exact artifact, component pin, and seed service change
before the operator installs it. Keep the private target paths in operator
configuration. Set `HOLDER_PACKAGE` to the verified package directory,
`HOLDER_DATA` to the existing Runtime data root, `HOLDER_RUNTIME` to the existing
Runtime executable, and `HOLDER_HELPERS` to its approved frozen helper root.
Prepare a candidate manifest beside the package while the service runs:

```bash
set -euo pipefail
export HOLDER_PACKAGE HOLDER_DATA HOLDER_RUNTIME HOLDER_HELPERS
(cd "$HOLDER_PACKAGE" && sha256sum -c SHA256SUMS)
python3 -I -S - <<'PY'
import hashlib, json, os
from pathlib import Path
package, data = Path(os.environ['HOLDER_PACKAGE']), Path(os.environ['HOLDER_DATA'])
runtime = Path(os.environ['HOLDER_RUNTIME'])
assert all(p.is_dir() and not p.is_symlink() for p in (package, data, Path(os.environ['HOLDER_HELPERS'])))
def sha(path):
    assert path.is_file() and not path.is_symlink()
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(chunk)
    return digest.hexdigest()
accepted = json.loads((package / 'accepted-build.json').read_bytes())
original_runtime = next(item for item in accepted['binaries'] if item['name'] == 'elastos')
assert sha(runtime) == original_runtime['sha256'] and runtime.stat().st_size == original_runtime['size']
assert os.environ['HOLDER_HELPERS'] == accepted['helper_root']
proof = json.loads((package / 'holder-idle.json').read_bytes())
build = json.loads((package / 'provider-build.json').read_bytes())
pin = json.loads((package / 'installed-component.json').read_bytes())
assert proof['passed'] and proof['fixture_removed']
assert (proof['provider_commit'], proof['provider_tree']) == (build['source_commit'], build['source_tree'])
assert (pin['source_commit'], pin['source_tree']) == (build['source_commit'], build['source_tree'])
assert pin['management'] == 'operator-managed-local-overlay'
assert sha(package / 'provider-build.json') == pin['provider_build_sha256']
assert sha(package / 'ipfs-provider') == build['sha256']
assert (package / 'ipfs-provider').stat().st_size == build['size']
manifest_path = data / 'components.json'
assert manifest_path.is_file() and not manifest_path.is_symlink()
manifest_bytes = manifest_path.read_bytes()
manifest = json.loads(manifest_bytes)
entry = manifest['external']['ipfs-provider']
assert entry['install_path'] == pin['entry']['install_path'] == 'bin/ipfs-provider'
assert entry['provider_runtime'] == pin['entry']['provider_runtime']
assert entry['provider_runtime']['role'] == 'provider'
assert entry['provider_runtime']['runtime_abi'] == 'elastos.provider-stdio/v1'
assert 'linux-amd64' in entry['platforms']
new_pin = pin['entry']['platforms']['linux-amd64']
assert new_pin['checksum'] == 'sha256:' + build['sha256'] and new_pin['size'] == build['size']
entry['platforms']['linux-amd64'] = dict(new_pin)
candidate = package / 'components.candidate.json'
with candidate.open('x') as stream:
    stream.write(json.dumps(manifest, indent=2) + '\n')
candidate.chmod(0o600)
receipt = package / 'components.candidate.receipt.json'
with receipt.open('x') as stream:
    json.dump({'previous_sha256': hashlib.sha256(manifest_bytes).hexdigest(),
               'candidate_sha256': sha(candidate)}, stream)
    stream.write('\n')
receipt.chmod(0o600)
print('Runtime parity and provider manifest candidate verified')
print('Candidate manifest SHA-256:', sha(candidate))
PY
```

Review the candidate diff against the installed manifest. Only the existing
Linux IPFS platform pin changes. It keeps the install path, checksum, and size;
the local overlay receipt replaces the old CID, URL, and release fetch path.
Record that reviewed candidate SHA-256 in the protected operator approval, then
export it as `HOLDER_APPROVED_MANIFEST_SHA256` for the replacement step.
Keep one approved rollback set containing the old provider binary and manifest, with their hashes, reason, and expiry in the
private lifecycle inventory. The installed data root, Kubo binary and repo,
identity, provider config, publication receipts, and helper tree stay in place.

After approval, stop the existing holder service with its established service
manager. Replace `$HOLDER_DATA/bin/ipfs-provider` with the verified binary at
mode 0700, and replace `$HOLDER_DATA/components.json` with the candidate at
mode 0600. Stage each replacement beside its destination, verify its hash,
then rename it into place while the service is stopped. Start the same service
with its existing arguments and environment. Verify the installed pin:

Use the established service manager to stop the approved service first, and
confirm its Runtime and provider processes have exited. With the verified
rollback retained, these commands stage exclusive files beside each target
and replace only the provider and manifest:

```bash
umask 077
python3 -I -S - <<'PY'
import hashlib, json, os, stat
from pathlib import Path
package, data = Path(os.environ['HOLDER_PACKAGE']), Path(os.environ['HOLDER_DATA'])
pin = json.loads((package / 'installed-component.json').read_bytes())
receipt_path = package / 'components.candidate.receipt.json'
assert receipt_path.is_file() and not receipt_path.is_symlink()
receipt = json.loads(receipt_path.read_bytes())
approved = os.environ['HOLDER_APPROVED_MANIFEST_SHA256']
assert len(approved) == 64 and approved == receipt['candidate_sha256']
assert hashlib.sha256((data / 'components.json').read_bytes()).hexdigest() == receipt['previous_sha256']
def replace(source, destination, mode, expected=None):
    parent = destination.parent
    assert parent.is_dir() and not parent.is_symlink()
    for path in (parent, destination):
        st = path.lstat()
        assert st.st_uid == os.geteuid() and not stat.S_ISLNK(st.st_mode)
        assert not st.st_mode & 0o022
    content = source.read_bytes()
    if expected: assert hashlib.sha256(content).hexdigest() == expected
    staged = parent / (destination.name + '.holder-candidate')
    fd = os.open(staged, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, mode)
    try:
        with os.fdopen(fd, 'wb') as stream:
            stream.write(content)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(staged, destination)
        directory = os.open(parent, os.O_RDONLY | os.O_DIRECTORY)
        try: os.fsync(directory)
        finally: os.close(directory)
    finally:
        if staged.exists(): staged.unlink()
replace(package / 'ipfs-provider', data / 'bin/ipfs-provider', 0o700,
        pin['entry']['platforms']['linux-amd64']['checksum'].removeprefix('sha256:'))
replace(package / 'components.candidate.json', data / 'components.json', 0o600, approved)
print('Approved provider and manifest replacement complete')
PY
```

If either replacement fails, keep the service stopped and restore both files
from the verified rollback. Then restart through the same service manager and
verify the installed pin:

```bash
ELASTOS_DATA_DIR="$HOLDER_DATA" ELASTOS_COMPONENTS_JSON="$HOLDER_DATA/components.json" \
  "$HOLDER_HELPERS/scripts/installed-provider-verify.sh" --require-verified ipfs-provider
sha256sum "$HOLDER_DATA/bin/ipfs-provider" "$HOLDER_RUNTIME"
```

Match the running provider executable to the installed provider hash and the
Runtime hash to the accepted receipt. Record the intentional install restart.
Then let Kubo reach the normal production idle threshold. Observe the watcher
stop, removed coordination file, old process exit, and closed API socket. Keep
the holder service and provider processes running. The first operation after
the stop is an isolated consumer's typed Content fetch over Carrier;
preparation and health calls come after that read. Use a separate full-serve
consumer with holder availability and its public ticket, then its private
`/api/provider/content/fetch` operation for the complete release binary. Match
those bytes to the signed release hash. Keep the consumer's local IPFS provider
and Kubo absent, and record the selected holder identity and public ticket in
its read receipt. The public `/content/:cid` route has a
smaller rendering bound; use it only for a smaller generic file in the second
idle cycle. Verify fresh Kubo process identities and unchanged service and
provider identities. Use isolated consumer state and stop its test processes
when finished.

Record safe run, artifact, source, hash, and installed idle-recovery results in
#89. Keep private target paths and raw operator evidence in their existing
custody. Remove the rollback set when this installed acceptance gate closes.
This holder qualification supports the canary journey; broader release and
installed Home gates keep their own acceptance evidence.


## Gateway Runtime and provider installation after qualification

Runtime selects the gateway role and sends it through its verified private
provider Init. This upgrade installs Runtime, `ipfs-provider`, and their full
helper source from one reviewed source tree. The earlier provider-only procedure
remains the accepted idle-recovery installation. The new gateway role needs
explicit approval for the Runtime upgrade and the reviewed manifest candidate.

After the source merges into `develop`, use the Canary Linux publisher workflow
on its workflow-only branch. Supply `source_commit`, `source_tree`, and
`source_ci_run` from the exact merged source and its completed successful CI.
Select `always_on=true` and supply `lifecycle_proof_run` from a completed
successful Canary Linux holder `workflow_dispatch` run on the reviewed task
branch below for that same source tree. The fixture can run
before merge; the lifecycle source commit can differ from the package commit
only when its tree is identical. Dispatch the holder workflow with that exact
reviewed candidate commit and tree. The package source separately requires
merged ancestry and successful same-source CI. The builder checks these
identities, the authenticated lifecycle artifact digest, real idle time, first
complete Carrier read and cleanup. Pull-request runs remain development evidence.
The same admission functions qualify inert refused cases before artifact use.

```bash
gh workflow run canary-holder-linux.yml --repo Elacity/elastos-runtime \
  --ref fix/89-holder-always-on \
  -f provider_commit="$GATEWAY_TESTED_SOURCE_COMMIT" \
  -f provider_tree="$GATEWAY_SOURCE_TREE" \
  -f package_merged=false
```

Use the exact reviewed fixture commit for `GATEWAY_TESTED_SOURCE_COMMIT`.
Wait for its completed successful run and record that run ID in
`GATEWAY_LIFECYCLE_RUN` before dispatching the publisher:

```bash
gh workflow run canary-publisher-linux.yml --repo Elacity/elastos-runtime \
  --ref fix/89-canary-inputs \
  -f source_commit="$GATEWAY_SOURCE_COMMIT" \
  -f source_tree="$GATEWAY_SOURCE_TREE" \
  -f source_ci_run="$GATEWAY_SOURCE_CI_RUN" \
  -f lifecycle_proof_run="$GATEWAY_LIFECYCLE_RUN" \
  -f always_on=true
```

The original frozen dispatch defaults support the earlier publisher rehearsal.
A new source package requires the explicit lifecycle mode. Read the approved
helper root from the authenticated approval and build receipts; the operator
installs the complete `source.tar.gz` there as the existing service owner.

Download the exact completed run and artifact recorded in the operator approval,
using the new private transfer directory and outer archive verification in step
5. Require the publisher workflow path, workflow-only branch, `completed` and
`success`. Check the approved GitHub artifact digest and outer archive hash
before extraction. Refuse archive links, absolute paths and parent traversal.
Verify `SHA256SUMS` and these public package records:

- `receipts/admission.json` binds the merged source, successful source CI,
  helper-root approval and requested lifecycle mode.
- `receipts/build.json` binds both package binary hashes and sizes, the approved
  helper root, source parity, version, publication tests and Linux ABI.
- `receipts/lifecycle-admission.json` binds the separate source qualification.
  Its `tested_runtime` and `tested_provider` hashes belong to the CI proof at
  its CI helper root. `seed_bytes_lifecycle_tested=false` in the build receipt
  states that the package binaries were rebuilt at the approved seed root.
- The three `receipts/tested-*.json` files retain the authenticated lifecycle
  observations and binary build records. They prove real elapsed idle time,
  unchanged Kubo generation, the first complete Carrier read and cleanup.
- `installed-component.json` binds the package IPFS checksum and size to
  `linux-amd64`, its native provider ABI, both source identities and the hash
  of `receipts/build.json`. Its local overlay omits old fetch provenance.

Prepare the manifest candidate from the actual installed manifest bytes.
Preserve its other entries, profiles, provider ABI and install paths. Replace
only the existing Linux IPFS platform pin with the package pin; remove its stale
CID, URL and release fetch path. Keep the new Runtime hash in the installation
receipt. The source-tree `components.json` remains source provenance and does
not replace the operator's installed manifest. Review the candidate diff, bind
its hash and the previous manifest hash in a receipt, and obtain approval for
that exact candidate and both binary hashes before the service window.

Record the current installed and running Runtime/provider hashes, helper-source
identity, manifest bytes and service configuration. Keep one approved rollback
set for the previous Runtime, provider, manifest and helper source, with a size,
reason and cleanup condition in the private lifecycle inventory. Include only
intentional release files; keep the existing data root, identity, provider
configuration, Kubo binary/repo and publication receipts in place. Check the
whole staging and rollback size against the disk floor before copying.

Stage both verified binaries beside their stable destinations. Extract the
complete helper archive into a new empty sibling directory, with the existing
service owner and private root mode; preserve reviewed executable modes. Verify
every helper file against `receipts/source-files.json`. Stage the approved
manifest candidate separately. After approval, stop the service through its
established manager and confirm its owned Runtime/provider processes exit.
Replace the full helper tree as a unit at its exact embedded root, then replace
the binaries and manifest from the verified staged files. Keep the service
stopped if a replacement fails, restore the complete approved rollback set,
and verify its parity before restart. Start the same service with its retained
gateway arguments and environment.

Compare the installed and running Runtime/provider hashes and sizes with
`receipts/build.json`, verify the installed IPFS pin with the reviewed
`installed-provider-verify.sh --require-verified ipfs-provider`, and recheck the
helper-source hashes. Confirm that the provider received the Runtime-owned
Gateway role. Let Kubo remain idle for more than 600 seconds without content,
health or preparation requests. Observe its unchanged process identity and
unchanged `last_used` coordinates locally. The first operation after idle is
an isolated consumer's typed Content fetch over Carrier for the complete release
binary. Verify its signed hash, full size and version, and record read latency
plus unchanged Kubo, Runtime and provider generations. Keep the consumer's
local IPFS provider and Kubo absent. The earlier frozen-user recovery receipt
remains separate evidence for default-user idle stop and restart behavior.

Verify the public gateway and current publication after restart. Stop all
isolated consumer processes and retain their safe receipts under the approved
custody. Record exact package source, artifact, installed parity and idle/read
results in #89. Remove the rollback set when the installed acceptance gate
closes.
