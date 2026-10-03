# Operator M1/M2 canary procedure

This package is for the operator-approved first canary hop in issue #89.
The operator owns key conversion, signing, seed deployment and Mac execution.
The CI builder supplies public inputs and source receipts. The test uses an
isolated Mac HOME; the existing Mac account and Home data stay in place.

## Approved input identities

- Repository: `Elacity/elastos-runtime`.
- Source: `5814b06b0b5e1f42d05ab60002af9c548ec9d9b2`.
- Source tree: `72785a23bbed468f3460acbe998e99bb74c55906`.
- Source CI: <https://github.com/Elacity/elastos-runtime/actions/runs/37073055811>.
- Native builder: <https://github.com/Elacity/elastos-runtime/actions/runs/37084288629>.
- Draft versions: V1 `0.8.0-alpha.1`, V2 `0.8.0-alpha.2`; channel `canary`.
- Origin: `https://elastos.elacitylabs.com`.
- Public signer: `did:key:z6MkgwHd2BCWe1jHMXPiR6H1q1RFPcv1YzhMbK5G1kBarbfe`.
- The public holder node/ticket are retained in `public-baseline/carrier-bootstrap.json`.
  The holder's transport DID is separate from the release signer.
- Reviewed signer SHA-256: `e8904cfb98a076544264117dbc92e7fbeae11f9cf9173fa07e1782595381d5c2`.
- Mac Python: `/opt/homebrew/Cellar/python@3.12/3.12.13_2/Frameworks/Python.framework/Versions/3.12/bin/python3.12`,
  SHA-256 `fe46716a94d8efa4514feb3c39ba3e270deee2187556986f6ddcff54aba7bb9a`.
- Mac OpenSSL: `/opt/homebrew/Cellar/openssl@3/3.6.3/bin/openssl`,
  SHA-256 `5d8f84484b7317ec5639ce68ccecc1d6f565ca6df483c8ae731e25265d83466d`.

The policy drafts contain the exact unsigned-manifest hashes and develop pin.
Anders approves the versions, source/tool pins, custody paths and seed inputs
before signing. A changed develop head requires renewed policy approval.
Stable signing retains the version-tag-on-main gate.

## 1. Download and verify

Use the completed **handoff packaging run and artifact name linked in #89**.
Set `HANDOFF_RUN` and `HANDOFF_ARTIFACT` to those exact values. Download to a
new stable directory with at least 15% free disk after extraction:

```bash
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
existing seed Runtime's actual public Publisher receipt, including prior head
and release CIDs. If they differ, ask the key-free builder to re-prepare V1
against the actual receipt. Preserve the existing receipt and holder identity.
Canonical Runtime derives its Publisher state path from its selected data root;
`ELASTOS_PUBLISH_STATE_DIR` alone does not select that root.

Create a protected custody directory **outside INPUTS**. Install only the
reviewed signer there. Replace the policy's `/OPERATOR/` paths with canonical
absolute paths to the installed signer and operator-held PEM; keep all ancestor
directories protected from group/other writes. The custodian account owns the
single-link key file with mode 0600. Python/OpenSSL pins above were qualified for
Anders's Mac account; a different custodian account qualifies ownership anew.

```bash
CUSTODY="$HOME/.local/share/elastos-canary-custody"
test ! -e "$CUSTODY"
mkdir -p "$CUSTODY"
chmod 700 "$CUSTODY"
install -m 600 "$INPUTS/release-signer.py" "$CUSTODY/release-signer.py"
cp "$INPUTS/policy-drafts/unsigned-V1.json" "$CUSTODY/V1-policy.json"
chmod 600 "$CUSTODY/V1-policy.json"
PINNED_PYTHON='/opt/homebrew/Cellar/python@3.12/3.12.13_2/Frameworks/Python.framework/Versions/3.12/bin/python3.12'
PINNED_OPENSSL='/opt/homebrew/Cellar/openssl@3/3.6.3/bin/openssl'
INSTALLED_SIGNER="$CUSTODY/release-signer.py"
PEM_KEY="$CUSTODY/maintainer-ed25519.pem"
shasum -a 256 "$INSTALLED_SIGNER" "$PINNED_PYTHON" "$PINNED_OPENSSL"
"$PINNED_PYTHON" --version
"$PINNED_OPENSSL" version
```

The operator edits and approves `V1-policy.json`: `tool.path` becomes
`INSTALLED_SIGNER`, `key_path` becomes `PEM_KEY`; all other pins, manifest hash
and quotas come from the reviewed draft. Compare its `develop_oid` with
`gh api repos/Elacity/elastos-runtime/git/ref/heads/develop --jq .object.sha`.

## 3. Operator-only hex-to-PEM conversion

Set `HEX_KEY` to the operator's existing protected **32-byte Ed25519 seed stored
as 64 hex characters**. The key stays on this Mac under custody. The following
operator command reads it directly, confirms the public DID and creates one
exclusive mode-0600 PEM. It passes secret bytes through OpenSSL's stdin, with
no secret command argument, environment value, output or intermediate key file.
The DER form is PKCS#8 Ed25519 (RFC 8410).

```bash
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

The copied Runtime needs `scripts/publish-release.sh` and its reviewed Python
helper at the embedded root even when it imports an already signed set.
`--dry-run` returns before that helper check; use `--preflight-only` as well.
If the existing seed Runtime differs from the reviewed candidate, the operator
updates the intentional Runtime/helper files under Anders's approval and
restarts the existing instance with its retained data root, identity, holder,
provider configuration and service arguments. Verify binary parity after
restart. Use the existing publication instance; its one current signed set is
replaced at the first approved new-key canary import.

On the seed, set `SEED_RUNTIME`, `SIGNED_SET`, and `IPFS_PROVIDER` to their
verified absolute paths. Run in the existing instance's approved HOME/data-root
environment. For V1 and V2 alike:

```bash
V1='0.8.0-alpha.1'
DID='did:key:z6MkgwHd2BCWe1jHMXPiR6H1q1RFPcv1YzhMbK5G1kBarbfe'
"$SEED_RUNTIME" publish-release --version "$V1" --channel canary \
  --signed-publication "$SIGNED_SET" --publisher-did "$DID" \
  --dry-run --allow-signer-rotation
"$SEED_RUNTIME" publish-release --version "$V1" --channel canary \
  --signed-publication "$SIGNED_SET" --publisher-did "$DID" \
  --ipfs-provider-bin "$IPFS_PROVIDER" --preflight-only --allow-signer-rotation
"$SEED_RUNTIME" publish-release --version "$V1" --channel canary \
  --signed-publication "$SIGNED_SET" --publisher-did "$DID" \
  --ipfs-provider-bin "$IPFS_PROVIDER" --allow-signer-rotation
```

The first import asks for the complete DID to change the saved public pin.
Retain the committed public Publisher receipt and exact signed head/release
CIDs. Verify public `release-head.json`, `release.json`, `install.sh` and
bootstrap bytes against the approved set and holder. Preserve the chain;
publication failure restores the prior served set rather than resetting state.

## 6. M1 on the isolated Mac HOME

Keep one stable test HOME across both steps. Use the frozen installer already
verified against **signed V1**. This is the accepted CLI-only first hop;
normal Home acceptance still has the signed capsule-delivery gate in #217.

```bash
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
```

Record the selected source, config/data/support fingerprints and exact signed
receipt before M2. Run the agreed disposable-key tampered-binary and wrong-signer
refusal fixture in its separate test HOME, with the frozen trust pin, preserving
the positive real-key HOME. Production signing admits valid sets only. The
complete positive/refusal CI rehearsal is linked in #89; operator receipts are
separate acceptance evidence.

## 7. Finalize, sign and publish V2

After V1 is public, the key-free builder verifies its signed public envelopes
and committed predecessor CIDs, then produces a **metadata-only V2 finalization
artifact** from retained N2. This makes no native rebuild and uses no real key.
The artifact carries the new `signing-input.json` SHA-256 and refreshed policy.
Anders approves that exact hash and source/develop/tool pins. V2's final policy
replaces the deliberately unusable provisional policy.

Download and verify that completed artifact as in step 1. Set `V2_INPUTS` to
its final read-only unsigned root. Install its approved policy at
`$CUSTODY/V2-policy.json`, with the same custody tool/key paths. Then:

```bash
SIGNED_V2="$CUSTODY/signed-V2"
env -i "$PINNED_PYTHON" -I -S "$INSTALLED_SIGNER" \
  --policy "$CUSTODY/V2-policy.json" \
  --input-root "$V2_INPUTS" --output-root "$SIGNED_V2"
cmp "$SIGNED_V2/install.sh" "$SIGNED_V1/install.sh"
(cd "$SIGNED_V2" && shasum -a 256 *) > "$CUSTODY/signed-V2-SHA256SUMS"
scp -r "$SIGNED_V2" "$CUSTODY/signed-V2-SHA256SUMS" "$SEED_ALIAS:$SEED_STAGE/"
```

Repeat seed dry-run, preflight and import from step 5 with V2 version
`0.8.0-alpha.2` and its verified signed-set path. Retain the committed V2 receipt.

## 8. Plain Carrier M2, preservation and repeat

Use the same Mac test HOME and frozen installation. These plain commands select
its saved Carrier source; retain their output and transport evidence:

```bash
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
