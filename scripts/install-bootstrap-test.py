#!/usr/bin/env python3
"""Offline checks of the actual curl|bash installer functions; stdlib only.

python3 scripts/install-bootstrap-test.py --bash /bin/bash
Optional --publisher-fixtures DIR verifies retained release-head.json/release.json
bytes. Capture those separately with a request/hash receipt; this test never fetches.
"""

import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shlex
import signal
import subprocess
import tempfile
import time
import unittest
from unittest import mock


INSTALLER = Path(__file__).with_name("install.sh")
SOURCE = INSTALLER.read_text()
HELPERS = SOURCE.split("# ── Parse args", 1)[0]
PYTHON = SOURCE.split("<<'PY_ED25519'\n", 1)[1].split("\nPY_ED25519", 1)[0]
CRYPTO = {"__name__": "installer_test"}
exec(compile(PYTHON, str(INSTALLER) + ":embedded-verifier", "exec"), CRYPTO)
PROCESS_SOURCE = SOURCE.split("<<'PY_RUNTIME_CONTROL'\n", 1)[1].split("\nPY_RUNTIME_CONTROL", 1)[0]
PROCESSES = {"__name__": "installer_test"}
exec(compile(PROCESS_SOURCE, str(INSTALLER) + ":runtime-control", "exec"), PROCESSES)
PUBLISHER_DID = "did:key:z6MkrFPDgDi98Ek6AFHM3VT9bVJytnDf5mfHAV6gyrD5frYj"


def binding_fixture():
    # Fixed OpenSSL Ed25519 vectors from the disposable seed [7; 32]. This is
    # a test identity. Running these stdlib tests needs neither a key nor OpenSSL.
    did = "did:key:z6MkvDqGT54cXesYGvABpF1UapVNwjCqRcafi4Px6Thv5T3Z"
    payload = {"schema": "elastos.release/v1", "version": "0.7.1", "channel": "stable",
               "platforms": {"x86_64-linux": {
                   "binary": {"cid": "binary-a", "sha256": "a" * 64},
                   "components": {"cid": "components", "sha256": "b" * 64}}}}
    first = {"payload": payload, "signer_did": did,
             "signature": "e976be583f98da06863071e4f2006dc2ea97fd77451fbc278ef23fcc3e1f97bad27abe6f617c11b262994838b49896b2ded44b57531f62d3bcea70e2cefd2b06"}
    second = copy.deepcopy(first)
    second["payload"]["platforms"]["x86_64-linux"]["binary"]["cid"] = "binary-b"
    second["signature"] = "b3cff75e82eb740d0112fb5e58568badc6d99586f800fbd8a602ba1491ff896ff816e46a29178c7a9ed4a0689e3355501e88602ce26bb5e588b05f835c207707"
    head = {"payload": {"schema": "elastos.release.head/v1", "version": "0.7.1", "channel": "stable",
                        "latest_release_cid": "release-a",
                        "release_sha256": "3dcb060f99a9b300ace45ae82ac722189a9740a349fa631bbd7eb1fd9e0dcd36"},
            "signer_did": did,
            "signature": "8a248801f576beb6b053fa2f52637b2d3087f2804e44434c4d18abea47007dd24e81781f8c390d414d1b8b34055a06b81f0bd78dc83afa2ce152b59a8fce1101"}
    encode = lambda value: json.dumps(value, sort_keys=True, separators=(",", ":")).encode() + b"\n"
    return did, encode(head), encode(first), encode(second)


TEST_SEED = bytes([7]) * 32
BASE58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


def encode_point(point):
    x, y, z, _ = point
    inverse = pow(z, CRYPTO["FIELD"] - 2, CRYPTO["FIELD"])
    x, y = x * inverse % CRYPTO["FIELD"], y * inverse % CRYPTO["FIELD"]
    return (y | (x & 1) << 255).to_bytes(32, "little")


def sign_envelope(payload, domain, seed=TEST_SEED):
    """RFC 8032 signing over the installer's own verifier arithmetic; disposable test seed only."""
    expanded = hashlib.sha512(seed).digest()
    scalar = int.from_bytes(expanded[:32], "little") & ((1 << 254) - 8) | 1 << 254
    public = encode_point(CRYPTO["point_mul"](scalar, CRYPTO["BASE"]))
    canonical = json.dumps(payload, separators=(",", ":"), sort_keys=True,
                           ensure_ascii=False, allow_nan=False).encode("utf-8")
    message = hashlib.sha256(domain.encode("utf-8") + b"\0" + canonical).digest()
    nonce = int.from_bytes(hashlib.sha512(expanded[32:] + message).digest(), "little") % CRYPTO["ORDER"]
    commitment = encode_point(CRYPTO["point_mul"](nonce, CRYPTO["BASE"]))
    challenge = int.from_bytes(hashlib.sha512(commitment + public + message).digest(), "little") % CRYPTO["ORDER"]
    signature = commitment + ((nonce + challenge * scalar) % CRYPTO["ORDER"]).to_bytes(32, "little")
    number, encoded = int.from_bytes(b"\xed\x01" + public, "big"), ""
    while number:
        number, digit = divmod(number, 58)
        encoded = BASE58[digit] + encoded
    return {"payload": payload, "signer_did": "did:key:z" + encoded, "signature": signature.hex()}


def encode_envelope(envelope):
    return json.dumps(envelope, sort_keys=True, separators=(",", ":")).encode() + b"\n"


# A harmless stand-in for the Runtime binary: it records every command it is
# asked to run and answers only the calls the installer is expected to make.
RUNTIME_STUB = b'''#!/bin/bash
printf '%s\\n' "$*" >> "${ELASTOS_TEST_CALLS:?fixture call log}"
case "${1:-}" in
    --version) echo "elastos 0.7.1" ;;
    principal-root-upgrade|setup|home) exit 0 ;;
    *) exit 97 ;;
esac
'''
COMPONENTS = json.dumps({"schema": "elastos.components/v1", "capsules": {}, "external": {},
                         "profiles": {}}, sort_keys=True).encode() + b"\n"
BOOTSTRAP = (b'{"schema":"elastos.carrier.bootstrap/v1","role":"publisher",'
             b'"ticket":"fixture-ticket","node_id":"fixture-node"}\n')


def installable_fixture(runtime=RUNTIME_STUB, components=COMPONENTS, version="0.7.1"):
    """Deterministic signed head/release advertising the fixture Runtime stub and manifest."""
    release_payload = {
        "schema": "elastos.release/v1", "version": version, "channel": "stable",
        "released_at": 1, "prev_release_cid": None,
        "platforms": {"x86_64-linux": {
            "binary": {"cid": "binary-a", "sha256": hashlib.sha256(runtime).hexdigest(), "size": len(runtime)},
            "components": {"cid": "components-a", "sha256": hashlib.sha256(components).hexdigest(),
                           "size": len(components)}}}}
    signed_release = sign_envelope(release_payload, "elastos.release.v1")
    release = encode_envelope(signed_release)
    head_payload = {
        "schema": "elastos.release.head/v1", "version": version, "channel": "stable",
        "latest_release_cid": "release-a", "release_sha256": hashlib.sha256(release).hexdigest(),
        "signer_did": signed_release["signer_did"], "updated_at": 1, "prev_head_cid": None}
    head = encode_envelope(sign_envelope(head_payload, "elastos.release.head.v1"))
    return signed_release["signer_did"], head, release


# Public key, message and signature from RFC 8032 section 7.1, tests 1, 2,
# 3 and SHA(abc). These fixed vectors exercise the implementation above.
# https://www.rfc-editor.org/rfc/rfc8032.html#section-7.1
VECTORS = [
    (
        "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
        "",
        "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e06522490155"
        "5fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
    ),
    (
        "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c",
        "72",
        "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da"
        "085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00",
    ),
    (
        "fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025",
        "af82",
        "6291d657deec24024827e69c3abe01a30ce548a284743a445e3680d7db5ac3ac"
        "18ff9b538d16f290ae67f760984dc6594a7c15e9716ed28dc027beceea1ec40a",
    ),
    (
        "ec172b93ad5e563bf4932c70e1245034c35467ef2efd4d64ebf819683467e2bf",
        "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a"
        "2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
        "dc2a4459e7369633a52b1bf277839a00201009a3efbf3ecb69bea2186c26b589"
        "09351fc9ac90b3ecfdfbc7c66431e0303dca179c138ac17ad9bef1177331a704",
    ),
]


def shell(script, *args):
    return subprocess.run(
        [OPTIONS.bash, "--noprofile", "--norc", "-s", "--", *map(str, args)],
        input=HELPERS + "\nALLOW_UNSIGNED=false\n" + script,
        text=True, capture_output=True, timeout=15,
    )


class SignatureTests(unittest.TestCase):
    def verify(self, public, message, signature):
        CRYPTO["verify_ed25519"](public, message, signature)

    def test_rfc8032_vectors(self):
        for number, vector in enumerate(VECTORS, 1):
            with self.subTest(vector=number):
                self.verify(*map(bytes.fromhex, vector))

    def test_altered_message_signature_and_key(self):
        for number, vector in enumerate(VECTORS, 1):
            public, message, signature = map(bytes.fromhex, vector)
            cases = [
                (public, message + b"x", signature),
                (public, message, bytes([signature[0] ^ 1]) + signature[1:]),
                (public[:-1] + bytes([public[-1] ^ 1]), message, signature),
                (bytes.fromhex(VECTORS[(number % len(VECTORS))][0]), message, signature),
            ]
            for index, case in enumerate(cases):
                with self.subTest(vector=number, mutation=index), self.assertRaises(ValueError):
                    self.verify(*case)

    def test_truncated_or_extended_inputs(self):
        public, message, signature = map(bytes.fromhex, VECTORS[0])
        for key, sig in [(public[:-1], signature), (public + b"x", signature),
                         (public, signature[:-1]), (public, signature + b"x")]:
            with self.subTest(key_length=len(key), sig_length=len(sig)), self.assertRaises(ValueError):
                self.verify(key, message, sig)

    def test_noncanonical_scalar(self):
        public, message, signature = map(bytes.fromhex, VECTORS[0])
        order = 2**252 + 27742317777372353535851937790883648493
        for scalar in [order, order + int.from_bytes(signature[32:], "little"), 2**256 - 1]:
            with self.subTest(scalar=scalar), self.assertRaisesRegex(ValueError, "scalar"):
                self.verify(public, message, signature[:32] + scalar.to_bytes(32, "little"))

    def test_invalid_and_noncanonical_points(self):
        public, message, signature = map(bytes.fromhex, VECTORS[0])
        # y >= p; x=0 encoded with sign=1; y=2 is outside the curve.
        for packed in [2**255 - 19, 2**255 - 1, 2**255 + 1, 2]:
            point = packed.to_bytes(32, "little")
            for key, sig in [(point, signature), (public, point + signature[32:])]:
                with self.subTest(point=packed, public=key == point), self.assertRaises(ValueError):
                    self.verify(key, message, sig)

    def test_small_order_public_keys_and_r(self):
        public, message, signature = map(bytes.fromhex, VECTORS[0])
        # Encodings of points of order 1, 4 and 2, respectively.
        for packed in [1, 0, 2**255 - 20]:
            point = packed.to_bytes(32, "little")
            for key, sig in [(point, signature), (public, point + signature[32:])]:
                with self.subTest(point=packed, public=key == point), self.assertRaisesRegex(ValueError, "Small-order"):
                    self.verify(key, message, sig)

    def test_pinned_publisher_did_conversion(self):
        self.assertEqual(
            CRYPTO["decode_did_key"](PUBLISHER_DID).hex(),
            "af41628c49d1321500bb1ff54af3f7563e1090b6235ddd38802339cc23608404",
        )
        for did in ["did:key:z", PUBLISHER_DID + "0", "did:key:z" + "1" * 34,
                    PUBLISHER_DID.replace("did:key:z", "did:key:m"), None,
                    "did:key:z" + "2" * 65]:
            with self.subTest(did=did), self.assertRaises(ValueError):
                CRYPTO["decode_did_key"](did)

    def test_envelope_digest_matches_publisher_utf8_vector(self):
        # Fixed digest independently checked with publish-release.sh's jq -cS
        # serialization, then SHA256(domain + NUL + those exact UTF-8 bytes).
        # Capture only the curve call to isolate envelope serialization. Real
        # signature acceptance is covered by the RFC and publisher fixtures.
        envelope = {
            "payload": {"z": {"β": "🌱", "a": [{"é": "café", "a": 1}]},
                        "a": "ElastOS\nHome", "m": "日本語"},
            "signer_did": PUBLISHER_DID,
            "signature": "00" * 64,
        }
        captured = []
        verifier = CRYPTO["verify_ed25519"]
        try:
            CRYPTO["verify_ed25519"] = lambda *args: captured.append(args)
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory, "unicode.json")
                path.write_text(json.dumps(envelope), encoding="utf-8")
                CRYPTO["verify_envelope"](path, "elastos.release.v1", PUBLISHER_DID)
        finally:
            CRYPTO["verify_ed25519"] = verifier
        self.assertEqual(len(captured), 1)
        self.assertEqual(captured[0][1].hex(),
                         "552b54638ce328bdf8101de06535ca821a8d00230028263d1f14a1c13844d71b")


class ShellTests(unittest.TestCase):
    def test_bootstrap_refresh_is_bash32_compatible(self):
        result = shell('''
PUBLISHER_GATEWAY=https://test.invalid
SOURCE_CONNECT_TICKET=old-ticket
PUBLISHER_NODE_ID=old-node
SOURCE_CONNECT_TICKET_EXPLICIT=false
PUBLISHER_NODE_ID_EXPLICIT=false
curl() { printf '%s\\n' '{"schema":"elastos.carrier.bootstrap/v1","role":"publisher","ticket":"fresh-ticket","node_id":"fresh-node"}'; }
refresh_source_bootstrap_from_publisher
[[ "$SOURCE_CONNECT_TICKET" == fresh-ticket && "$PUBLISHER_NODE_ID" == fresh-node ]]
''')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_bootstrap_failure_preserves_atomic_stamped_pair(self):
        for response in ["{}", '{"schema":"elastos.carrier.bootstrap/v1","role":"publisher","ticket":"partial"}',
                         '{"schema":"elastos.carrier.bootstrap/v1","role":"runtime","ticket":"wrong","node_id":"wrong"}']:
            result = shell('''
PUBLISHER_GATEWAY=https://test.invalid
SOURCE_CONNECT_TICKET=old-ticket
PUBLISHER_NODE_ID=old-node
SOURCE_CONNECT_TICKET_EXPLICIT=false
PUBLISHER_NODE_ID_EXPLICIT=false
curl() { printf '%s\\n' "$1"; }
''' .replace('"$1"', shlex.quote(response)) + '''
refresh_source_bootstrap_from_publisher
[[ "$SOURCE_CONNECT_TICKET" == old-ticket && "$PUBLISHER_NODE_ID" == old-node ]]
''')
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_explicit_bootstrap_pair_avoids_refresh(self):
        result = shell('''
PUBLISHER_GATEWAY=https://test.invalid
SOURCE_CONNECT_TICKET=explicit-ticket
PUBLISHER_NODE_ID=explicit-node
SOURCE_CONNECT_TICKET_EXPLICIT=true
PUBLISHER_NODE_ID_EXPLICIT=true
curl() { echo unexpected-network-call >&2; exit 91; }
refresh_source_bootstrap_from_publisher
[[ "$SOURCE_CONNECT_TICKET" == explicit-ticket && "$PUBLISHER_NODE_ID" == explicit-node ]]
''')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_empty_gateway_array_fails_with_installer_message(self):
        result = shell('GATEWAYS=()\nipfs_fetch test-cid /unused\n')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Failed to fetch CID", result.stderr)
        self.assertNotIn("unbound variable", result.stderr)

    def test_json_path_can_contain_quote_and_spaces(self):
        with tempfile.TemporaryDirectory(prefix="installer test's ") as directory:
            path = Path(directory, "metadata.json")
            path.write_text('{"version":"0.7.1"}')
            result = shell('json_get "$1" \'d["version"]\'\n', path)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "0.7.1")

    def test_release_identity_requires_matching_nonempty_strings(self):
        head = {"payload": {"schema": "elastos.release.head/v1", "version": "0.7.1", "channel": "stable"}}
        release = {"payload": {"schema": "elastos.release/v1", "version": "0.7.1", "channel": "stable"}}
        cases = [(head, release, True)]
        for field in ["version", "channel"]:
            for value in ["other", "", None, 7]:
                altered = copy.deepcopy(release)
                altered["payload"][field] = value
                cases.append((head, altered, False))
            missing = copy.deepcopy(head)
            missing["payload"].pop(field)
            cases.append((missing, release, False))
        malformed = copy.deepcopy(release)
        malformed["payload"]["schema"] = "wrong"
        cases.append((head, malformed, False))
        with tempfile.TemporaryDirectory() as directory:
            paths = [Path(directory, name) for name in ("head.json", "release.json")]
            for index, (first, second, accepted) in enumerate(cases):
                paths[1].write_text(json.dumps(second))
                first = copy.deepcopy(first)
                first["payload"]["release_sha256"] = hashlib.sha256(paths[1].read_bytes()).hexdigest()
                paths[0].write_text(json.dumps(first))
                result = shell('validate_release_identity "$1" "$2"\n', *paths)
                with self.subTest(case=index):
                    self.assertEqual(result.returncode == 0, accepted, result.stdout + result.stderr)


class InstallerSandbox:
    """Disposable HOME, data root, install dir, temp dir and logs for the complete installer.

    Only transport (curl) and platform reporting (uname) are replaced. Requests are
    answered from the response files present under responses/; any other request,
    and any request for a response that is absent, fails and is logged.
    """

    def __init__(self, head, release, did, system="Linux", machine="x86_64"):
        self.directory = tempfile.TemporaryDirectory(prefix="installer-sandbox-")
        self.root = Path(self.directory.name)
        self.did, self.system, self.machine = did, system, machine
        self.home = self.root / "home"
        self.data = self.home / "xdg-data/elastos"
        self.binary = self.home / ".local/bin/elastos"
        self.calls = self.root / "calls"
        self.responses = self.root / "responses"
        for path in (self.home, self.root / "tmp", self.responses, self.root / "mocks"):
            path.mkdir(parents=True)
        (self.root / "requests").write_text("")
        self.head_cid = "head-a"
        self.release_cid = json.loads(head)["payload"]["latest_release_cid"]
        platform_entry = json.loads(release)["payload"].get("platforms", {}).get("x86_64-linux", {})
        self.binary_cid = platform_entry.get("binary", {}).get("cid", "")
        self.components_cid = platform_entry.get("components", {}).get("cid", "")
        self.respond("release-head.json", head)
        self.respond("release.json", release)
        mocks = self.root / "mocks"
        (mocks / "uname").write_text('#!/bin/sh\ncase "$1" in -s) echo "$MOCK_SYSTEM";; -m) echo "$MOCK_MACHINE";; esac\n')
        (mocks / "curl").write_text('''#!/bin/sh
destination=""
while [ "$#" -gt 0 ]; do
    case "$1" in -o) destination="$2"; shift 2;; *) url="$1"; shift;; esac
done
printf '%s\\n' "$url" >> "$FIXTURES/requests"
case "$url" in
  */release-head.json|*/ipfs/"$FIXTURE_HEAD_CID") response=release-head.json;;
  */release.json|*/ipfs/"$FIXTURE_RELEASE_CID") response=release.json;;
  */artifacts/elastos-*|*/ipfs/"$FIXTURE_BINARY_CID") response=binary;;
  */artifacts/components-*.json|*/ipfs/"$FIXTURE_COMPONENTS_CID") response=components;;
  */.well-known/elastos/carrier-bootstrap.json*) response=bootstrap;;
  *) echo unexpected-request-blocked >&2; exit 93;;
esac
[ -f "$FIXTURES/responses/$response" ] || { echo artifact-request-blocked >&2; exit 93; }
if [ -n "$destination" ]; then cp "$FIXTURES/responses/$response" "$destination"; else cat "$FIXTURES/responses/$response"; fi
''')
        for path in mocks.iterdir():
            path.chmod(0o755)

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.directory.cleanup()

    def respond(self, name, data):
        (self.responses / name).write_bytes(data)

    def requests(self):
        return (self.root / "requests").read_text().splitlines()

    def run(self, *options, transport="publisher"):
        seen = len(self.requests())
        options = list(options) + (["--publisher-gateway", "https://test.invalid"] if transport == "publisher"
                                   else ["--gateway", "https://test.invalid", "--head-cid", self.head_cid])
        result = shell('''
export HOME="$1/home" XDG_DATA_HOME="$1/home/xdg-data" TMPDIR="$1/tmp" FIXTURES="$1" PATH="$1/mocks:$PATH"
export ELASTOS_PUBLISHER_GATEWAY="" ELASTOS_HEAD_CID="" ELASTOS_IPFS_GATEWAYS=""
export ELASTOS_SOURCE_CONNECT_TICKET="" ELASTOS_PUBLISHER_NODE_ID="" ELASTOS_INSTALL_ONLY=""
export ELASTOS_TEST_CALLS="$1/calls" MOCK_SYSTEM="$4" MOCK_MACHINE="$5"
export FIXTURE_HEAD_CID="$6" FIXTURE_RELEASE_CID="$7" FIXTURE_BINARY_CID="$8" FIXTURE_COMPONENTS_CID="$9"
exec "$2" --noprofile --norc "$3" "${@:10}"
''', self.root, OPTIONS.bash, INSTALLER, self.system, self.machine, self.head_cid, self.release_cid,
                       self.binary_cid, self.components_cid, "--maintainer-did", self.did, *options)
        return result, self.requests()[seen:]

    def runtime_calls(self):
        return self.calls.read_text().splitlines() if self.calls.exists() else []

    def home_state(self):
        """Bytes and mode of every file under HOME, so unchanged means identical."""
        state = {}
        for path in sorted(self.home.rglob("*")):
            key = str(path.relative_to(self.home))
            if path.is_symlink():
                state[key] = ("link", os.readlink(path))
            elif path.is_dir():
                state[key] = ("dir",)
            else:
                state[key] = ("file", path.stat().st_mode & 0o777, path.read_bytes())
        return state


def run_offline_installer(head, release, did, transport="publisher", system="Linux", machine="x86_64"):
    # Only transport and uname are replaced. The complete installer executes,
    # and every artifact request fails before installation or external access.
    with InstallerSandbox(head, release, did, system, machine) as sandbox:
        result, requests = sandbox.run(transport=transport)
        if sandbox.binary.exists():
            raise AssertionError("Offline fixture reached installation")
        return result, requests


class CompletionTests(unittest.TestCase):
    def run_completion(self, setup_exit=0, home_exit=0, install_only="false", terminal=False):
        with tempfile.TemporaryDirectory(prefix="installer-completion-") as directory:
            root = Path(directory)
            # Spaces catch accidental reliance on PATH or unquoted install paths.
            install_dir = root / "installed runtime"
            install_dir.mkdir()
            runtime = install_dir / "elastos"
            runtime.write_text("#!/bin/bash\n"
                               'printf "%s\\n" "$*" >> "$CALLS"\n'
                               'if [[ "${1:-}" == setup ]]; then\n'
                               '  if read -r unexpected; then exit 98; fi\n'
                               '  exit "$SETUP_EXIT"\n'
                               'fi\n'
                               '[[ -t 0 ]] || exit 99\n'
                               'exit "$HOME_EXIT"\n')
            runtime.chmod(0o755)
            command = HELPERS + "\nfinish_install\n"
            script = root / "completion.sh"
            script.write_text(command)
            calls = root / "calls"
            env = dict(os.environ, INSTALL_DIR=str(install_dir), INSTALL_ONLY=install_only,
                       CALLS=str(calls), SETUP_EXIT=str(setup_exit), HOME_EXIT=str(home_exit))
            argv = [OPTIONS.bash, "--noprofile", "--norc", str(script)]
            if terminal:
                import fcntl
                import termios
                master, slave = os.openpty()
                def controlling_terminal():
                    os.setsid()
                    fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
                try:
                    proc = subprocess.Popen([OPTIONS.bash, "--noprofile", "--norc", "-s"],
                                            stdin=subprocess.PIPE, stdout=slave, stderr=slave,
                                            env=env, pass_fds=(slave,), preexec_fn=controlling_terminal)
                    proc.stdin.write(command.encode())
                    proc.stdin.close()
                    import select
                    chunks = []
                    deadline = time.monotonic() + 10
                    while proc.poll() is None and time.monotonic() < deadline:
                        if select.select([master], [], [], 0.1)[0]:
                            chunks.append(os.read(master, 65536))
                    output = b"".join(chunks).decode(errors="replace")
                    if proc.poll() is None:
                        proc.kill()
                        proc.wait()
                        raise AssertionError("Terminal completion timed out: " + output)
                    status = proc.returncode
                finally:
                    os.close(master)
                    os.close(slave)
            else:
                result = subprocess.run(argv, input="remaining curl input\n", text=True,
                                        capture_output=True, env=env, timeout=10)
                status, output = result.returncode, result.stdout
            return status, calls.read_text().splitlines() if calls.exists() else [], output

    def test_headless_setup_does_not_consume_script_pipe_or_open_renderer(self):
        status, calls, output = self.run_completion()
        self.assertEqual((status, calls), (0, ["setup"]))
        self.assertIn("Home is installed", output)
        self.assertIn("installed\\ runtime/elastos", output)
        self.assertIn("home --browser", output)

    def test_setup_failure_stops_before_home_and_success_message(self):
        status, calls, output = self.run_completion(setup_exit=23)
        self.assertEqual((status, calls), (23, ["setup"]))
        self.assertNotIn("Home is installed", output)

    def test_install_only_skips_both_setup_and_home(self):
        for value in ["true", "1"]:
            status, calls, _ = self.run_completion(install_only=value)
            self.assertEqual((status, calls), (0, []))

    def test_terminal_install_sets_up_then_opens_home_with_tty(self):
        status, calls, _ = self.run_completion(terminal=True)
        self.assertEqual((status, calls), (0, ["setup", "home --browser"]))

    def test_home_failure_is_reported(self):
        status, calls, _ = self.run_completion(terminal=True, home_exit=24)
        self.assertEqual((status, calls), (24, ["setup", "home --browser"]))


class BindingTests(unittest.TestCase):
    def test_exact_signed_release_required_even_at_same_version(self):
        did, head, first, second = binding_fixture()
        with tempfile.TemporaryDirectory(prefix="installer-binding-") as directory:
            paths = [Path(directory, name) for name in ("head.json", "release.json")]
            paths[0].write_bytes(head)
            for name, candidate, accepted in [("matching", first, True), ("other signed release", second, False),
                                               ("whitespace", first + b" ", False)]:
                paths[1].write_bytes(candidate)
                result = shell('''
verify_signature "$1" elastos.release.head.v1 "$3"
verify_signature "$2" elastos.release.v1 "$3"
validate_release_identity "$1" "$2"
''', *paths, did)
                with self.subTest(case=name):
                    self.assertEqual(result.stdout.count("Signature verified"), 2, result.stderr)
                    self.assertEqual(result.returncode == 0, accepted, result.stderr)

    def test_missing_or_invalid_digest_fails_closed(self):
        _, head, release, _ = binding_fixture()
        with tempfile.TemporaryDirectory(prefix="installer-binding-") as directory:
            paths = [Path(directory, name) for name in ("head.json", "release.json")]
            paths[1].write_bytes(release)
            for value in [None, 7, "", "a" * 63, "G" * 64, "A" * 64, "0" * 64]:
                first = json.loads(head)
                first["payload"]["release_sha256"] = value
                paths[0].write_text(json.dumps(first))
                result = shell('validate_release_identity "$1" "$2"\n', *paths)
                with self.subTest(value=value):
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn("Release identity check failed", result.stderr)

    def test_binding_precedes_artifacts_on_publisher_and_cid_transports(self):
        did, head, first, second = binding_fixture()
        changed_byte = first.replace(b'"version":"0.7.1"', b'"version":"0.8.1"')
        self.assertNotEqual(first, changed_byte)
        for transport in ["publisher", "cid"]:
            for name, release, matching in [("matching", first, True), ("other signed release", second, False),
                                            ("whitespace", first + b" ", False), ("one byte", changed_byte, False)]:
                result, requests = run_offline_installer(head, release, did, transport)
                with self.subTest(transport=transport, case=name):
                    self.assertNotEqual(result.returncode, 0)
                    self.assertEqual(len(requests), 3 if matching else 2, result.stdout + result.stderr)
                    self.assertEqual(requests[0], "https://test.invalid/" + ("release-head.json" if transport == "publisher" else "ipfs/head-a"))
                    self.assertEqual(requests[1], "https://test.invalid/" + ("release.json" if transport == "publisher" else "ipfs/release-a"))
                    if matching:
                        self.assertEqual(requests[2], "https://test.invalid/" + ("artifacts/elastos-x86_64-linux" if transport == "publisher" else "ipfs/binary-a"))
                    else:
                        self.assertIn("Signature verification FAILED" if name == "one byte" else
                                      "Release envelope differs from the signed head", result.stderr)
        result, requests = run_offline_installer(head, first, did, system="Darwin", machine="arm64")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("No release available for platform: aarch64-darwin", result.stderr)
        self.assertEqual(len(requests), 2)

    def test_publisher_hashes_the_final_envelope_bytes_into_head_payload(self):
        source = INSTALLER.with_name("publish-release.sh").read_text()
        sha_helper = source[source.index("sha256() {"):source.index("\nfile_size() {")]
        envelope_write = source[source.index('echo "$RELEASE_JSON" > "${TMPDIR}/release.json"'):source.index('info "Publishing release.json to IPFS..."')]
        head_payload = source[source.index("HEAD_PAYLOAD=$(jq"):source.index('info "Signing release head..."')]
        with tempfile.TemporaryDirectory(prefix="publisher-binding-") as directory:
            result = shell(sha_helper + '''
TMPDIR="$1"
RELEASE_JSON='{"payload":{"note":"café"},"signature":"test"}'
CHANNEL=stable VERSION=0.7.1 RELEASE_CID=release-a RELEASE_OBJECT_CID=object-a
SIGNER_DID=test PREV_HEAD_CID=null
now_unix() { echo 1; }
''' + envelope_write + head_payload + '\nprintf "%s\\n" "$HEAD_PAYLOAD"\n', directory)
            self.assertEqual(result.returncode, 0, result.stderr)
            written = Path(directory, "release.json").read_bytes()
            payload = json.loads(result.stdout)
            self.assertTrue(written.endswith(b"\n"))
            self.assertEqual(payload["release_sha256"], hashlib.sha256(written).hexdigest())
            self.assertEqual(payload["latest_release_cid"], "release-a")


class InstallationTests(unittest.TestCase):
    """The complete installer against served artifact bytes inside one disposable sandbox."""

    REQUESTS = {
        "publisher": ["https://test.invalid/release-head.json", "https://test.invalid/release.json",
                      "https://test.invalid/artifacts/elastos-x86_64-linux",
                      "https://test.invalid/artifacts/components-x86_64-linux.json",
                      "https://test.invalid/.well-known/elastos/carrier-bootstrap.json?role=publisher"],
        "cid": ["https://test.invalid/ipfs/head-a", "https://test.invalid/ipfs/release-a",
                "https://test.invalid/ipfs/binary-a", "https://test.invalid/ipfs/components-a"],
    }

    def test_test_signer_reproduces_fixed_fixture_vectors(self):
        # The signer must agree with the fixed OpenSSL vectors that anchor binding_fixture(),
        # and every envelope it produces must pass the installer's own verifier.
        did, head, first, second = binding_fixture()
        for raw, domain in [(first, "elastos.release.v1"), (second, "elastos.release.v1"),
                            (head, "elastos.release.head.v1")]:
            envelope = json.loads(raw)
            signed = sign_envelope(envelope["payload"], domain)
            self.assertEqual((signed["signer_did"], signed["signature"]), (did, envelope["signature"]))
        signer, head, release = installable_fixture()
        self.assertEqual(signer, did)
        with tempfile.TemporaryDirectory(prefix="installer-signer-") as directory:
            for name, data, domain in [("head.json", head, "elastos.release.head.v1"),
                                       ("release.json", release, "elastos.release.v1")]:
                Path(directory, name).write_bytes(data)
                CRYPTO["verify_envelope"](Path(directory, name), domain, did)
            self.assertEqual(json.loads(head)["payload"]["release_sha256"], hashlib.sha256(release).hexdigest())

    def existing_installation(self, sandbox):
        sandbox.binary.parent.mkdir(parents=True)
        sandbox.binary.write_bytes(b"#!/bin/sh\necho previous runtime\n")
        sandbox.binary.chmod(0o755)
        files = {
            "components.json": b'{"schema":"elastos.components/v1","capsules":{},"note":"previous"}\n',
            "sources.json": b'{"schema":"elastos.trusted-sources/v1","note":"previous"}\n',
            "Users/alice/notes.txt": b"user data stays\n",
            "capsules/shell/cache.bin": b"cached capsule stays\n",
            "ElastOS/SystemServices/Publisher/release-head.json": b"previous head\n",
            "ElastOS/SystemServices/Publisher/release.json": b"previous release\n",
        }
        for name, data in files.items():
            (sandbox.data / name).parent.mkdir(parents=True, exist_ok=True)
            (sandbox.data / name).write_bytes(data)

    def assert_no_installation_effects(self, sandbox, before, result):
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn("Stopping verified Runtime processes", result.stdout)
        self.assertEqual(sandbox.runtime_calls(), [])
        self.assertEqual(sandbox.home_state(), before)
        self.assertEqual(list((sandbox.root / "tmp").iterdir()), [])

    def test_corrupt_artifacts_fail_before_changes_then_clean_rerun_installs(self):
        did, head, release = installable_fixture()
        cases = [
            ("truncated binary", RUNTIME_STUB[:-9], COMPONENTS, "binary"),
            ("wrong binary bytes", RUNTIME_STUB.replace(b"0.7.1", b"0.7.2"), COMPONENTS, "binary"),
            ("wrong components bytes", RUNTIME_STUB, COMPONENTS.replace(b"{}", b"{ }", 1), "components"),
        ]
        for transport in ["publisher", "cid"]:
            with InstallerSandbox(head, release, did) as sandbox:
                self.existing_installation(sandbox)
                sandbox.respond("bootstrap", BOOTSTRAP)
                before = sandbox.home_state()
                expected = self.REQUESTS[transport]
                for name, binary, components, stage in cases:
                    sandbox.respond("binary", binary)
                    sandbox.respond("components", components)
                    result, requests = sandbox.run("--install-only", transport=transport)
                    with self.subTest(transport=transport, case=name):
                        self.assertIn("SHA-256 mismatch", result.stderr)
                        if stage == "binary":
                            self.assertIn("Verifying binary SHA-256", result.stdout)
                            self.assertNotIn("Downloading components.json", result.stdout)
                            self.assertEqual(requests, expected[:3])
                        else:
                            self.assertIn("Verifying components.json SHA-256", result.stdout)
                            self.assertEqual(requests, expected[:4])
                        self.assert_no_installation_effects(sandbox, before, result)
                sandbox.respond("binary", RUNTIME_STUB)
                sandbox.respond("components", COMPONENTS)
                with self.subTest(transport=transport, case="clean rerun"):
                    self.assert_clean_install(sandbox, transport, did, head, release, before)

    def assert_clean_install(self, sandbox, transport, did, head, release, before):
        """A valid --install-only run in a sandbox that already holds an installation."""
        sandbox.calls.unlink(missing_ok=True)
        result, requests = sandbox.run("--install-only", transport=transport)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(requests, self.REQUESTS[transport])
        self.assertIn("Runtime installed:", result.stdout)
        self.assertNotIn("Setting up Home", result.stdout)
        advertised = json.loads(release)["payload"]["platforms"]["x86_64-linux"]
        installed = sandbox.binary.read_bytes()
        self.assertEqual(installed, RUNTIME_STUB)
        self.assertEqual(hashlib.sha256(installed).hexdigest(), advertised["binary"]["sha256"])
        self.assertTrue(sandbox.binary.stat().st_mode & 0o100)
        self.assertTrue(os.access(sandbox.binary, os.X_OK))
        self.assertFalse((sandbox.binary.parent / ".elastos.install.tmp").exists())
        manifest = (sandbox.data / "components.json").read_bytes()
        self.assertEqual(manifest, COMPONENTS)
        self.assertEqual(hashlib.sha256(manifest).hexdigest(), advertised["components"]["sha256"])
        calls = sandbox.runtime_calls()
        self.assertEqual(calls[0], "--version")
        self.assertRegex(calls[1], "^principal-root-upgrade --data-dir %s --backup-dir %s/backups/principal-root-upgrade-[0-9]+-[0-9]+$"
                         % (re.escape(str(sandbox.data)), re.escape(str(sandbox.data))))
        self.assertEqual(len(calls), 2, "setup and Home launch stay out of --install-only")
        sources = json.loads((sandbox.data / "sources.json").read_text())
        self.assertEqual(sources["schema"], "elastos.trusted-sources/v1")
        source = sources["sources"][0]
        self.assertEqual((source["publisher_dids"], source["channel"], source["installed_version"], source["install_path"]),
                         ([did], "stable", "0.7.1", str(sandbox.binary)))
        self.assertEqual(source["discovery_uri"],
                         "elastos://source/stable/" + hashlib.sha256(did.encode()).hexdigest()[:32])
        registration = (source["gateways"], source["head_cid"], source["connect_ticket"], source["publisher_node_id"])
        self.assertEqual(registration, (["https://test.invalid"], "", "fixture-ticket", "fixture-node")
                         if transport == "publisher" else ([], "head-a", "", ""))
        publisher = sandbox.data / "ElastOS/SystemServices/Publisher"
        self.assertEqual((publisher / "release-head.json").read_bytes(), head)
        self.assertEqual((publisher / "release.json").read_bytes(), release)
        after = sandbox.home_state()
        changed = {key for key in set(before) | set(after) if before.get(key) != after.get(key)}
        self.assertEqual(changed, {
            ".local/bin/elastos", "xdg-data/elastos/components.json", "xdg-data/elastos/sources.json",
            "xdg-data/elastos/ElastOS/SystemServices/Publisher/release-head.json",
            "xdg-data/elastos/ElastOS/SystemServices/Publisher/release.json"})
        self.assertEqual(list((sandbox.root / "tmp").iterdir()), [])

    def test_staged_executable_refusal_preserves_installation_then_valid_retry_installs(self):
        # These binaries carry the signed, advertised hash; only the staged
        # executable's own behavior can reject them, and that must happen
        # before the current binary is replaced.
        cases = [
            ("wrong version", RUNTIME_STUB.replace(b"elastos 0.7.1", b"elastos 0.6.0"),
             "version mismatch", ["--version"]),
            ("nonzero exit with expected version in output",
             RUNTIME_STUB.replace(b'echo "elastos 0.7.1" ;;', b'echo "elastos 0.7.1"; exit 3 ;;'),
             "failed its version check (exit 3)", ["--version"]),
            ("invalid executable", b"\x00\x01\x02 not an executable\n", "failed its version check", []),
        ]
        did, head, release = installable_fixture()
        for transport in ["publisher", "cid"]:
            with InstallerSandbox(head, release, did) as sandbox:
                self.existing_installation(sandbox)
                sandbox.respond("components", COMPONENTS)
                sandbox.respond("bootstrap", BOOTSTRAP)
                before = sandbox.home_state()
                for name, binary, message, expected_calls in cases:
                    _, served_head, served_release = installable_fixture(runtime=binary)
                    sandbox.respond("release-head.json", served_head)
                    sandbox.respond("release.json", served_release)
                    sandbox.respond("binary", binary)
                    sandbox.calls.unlink(missing_ok=True)
                    result, requests = sandbox.run("--install-only", transport=transport)
                    with self.subTest(transport=transport, case=name):
                        self.assertNotEqual(result.returncode, 0)
                        self.assertIn(message, result.stderr)
                        self.assertIn("the current installation was preserved", result.stderr)
                        self.assertEqual(requests, self.REQUESTS[transport][:4])
                        self.assertIn("Installing binary to", result.stdout)
                        self.assertNotIn("Installing components.json", result.stdout)
                        self.assertEqual(sandbox.runtime_calls(), expected_calls)
                        self.assertFalse((sandbox.binary.parent / ".elastos.install.tmp").exists())
                        self.assertEqual(sandbox.home_state(), before)
                        self.assertEqual(list((sandbox.root / "tmp").iterdir()), [])
                sandbox.respond("release-head.json", head)
                sandbox.respond("release.json", release)
                sandbox.respond("binary", RUNTIME_STUB)
                with self.subTest(transport=transport, case="valid retry"):
                    self.assert_clean_install(sandbox, transport, did, head, release, before)

    def test_signature_and_binding_failures_reject_before_artifacts_in_populated_installation(self):
        did, head, release = installable_fixture()
        unbound = json.loads(head)
        unbound["payload"]["release_sha256"] = "0" * 64
        unbound = encode_envelope(sign_envelope(unbound["payload"], "elastos.release.head.v1"))
        cases = [
            ("tampered release", head, release.replace(b'"version":"0.7.1"', b'"version":"0.8.1"'), did,
             "Signature verification FAILED", 2),
            ("head bound to other release", unbound, release, did, "Release envelope differs from the signed head", 2),
            ("foreign trust anchor", head, release, PUBLISHER_DID, "Signature verification FAILED", 1),
        ]
        for transport in ["publisher", "cid"]:
            for name, served_head, served_release, anchor, message, request_count in cases:
                with self.subTest(transport=transport, case=name), \
                        InstallerSandbox(served_head, served_release, anchor) as sandbox:
                    self.existing_installation(sandbox)
                    for response, data in [("binary", RUNTIME_STUB), ("components", COMPONENTS), ("bootstrap", BOOTSTRAP)]:
                        sandbox.respond(response, data)
                    before = sandbox.home_state()
                    result, requests = sandbox.run("--install-only", transport=transport)
                    self.assertIn(message, result.stderr)
                    self.assertEqual(requests, self.REQUESTS[transport][:request_count])
                    self.assert_no_installation_effects(sandbox, before, result)


class DataPathTests(unittest.TestCase):
    def test_three_platform_release_keys(self):
        for system, machine, expected in [("Linux", "x86_64", "x86_64-linux"),
                                          ("Linux", "aarch64", "aarch64-linux"),
                                          ("Darwin", "arm64", "aarch64-darwin")]:
            result = shell('''
SYSTEM="$1" MACHINE="$2"
uname() { case "$1" in -s) echo "$SYSTEM";; -m) echo "$MACHINE";; esac; }
detect_platform
printf '%s\\n' "$PLATFORM"
''', system, machine)
            with self.subTest(system=system, machine=machine):
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout.strip(), expected)

    def test_platform_paths_match_rust_dirs5_contract(self):
        # setup.rs and sources.rs use dirs5.0.1::data_dir. Mac ignores XDG;
        # Linux uses an absolute XDG path, otherwise HOME/.local/share.
        home = "/test/Home with ' spaces"
        for system, xdg, expected in [
            ("Darwin", "", home + "/Library/Application Support/elastos"),
            ("Darwin", "/different/xdg", home + "/Library/Application Support/elastos"),
            ("Linux", "", home + "/.local/share/elastos"),
            ("Linux", "relative/xdg", home + "/.local/share/elastos"),
            ("Linux", "/test/data/", "/test/data/elastos"),
        ]:
            result = shell('uname() { printf "%s\\n" "$SYSTEM"; }\nSYSTEM="$1"\ninstaller_data_dir "$2" "$3"\n',
                           system, home, xdg)
            with self.subTest(system=system, xdg=xdg):
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout.strip(), expected)

    def test_new_data_root_is_private_and_existing_root_is_preserved(self):
        block = SOURCE.split("# New Runtime data is private;", 1)[1].split("\n\n", 1)[0]
        block = block.split("\n", 1)[1]
        with tempfile.TemporaryDirectory(prefix="installer-data-mode-") as temp:
            for mask in ("0022", "0002", "0077"):
                with self.subTest(umask=mask):
                    data = Path(temp) / mask / "elastos"
                    script = 'umask "$1"\nDATA_DIR="$2"\n' + block
                    result = shell(script, mask, data)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(data.stat().st_mode & 0o777, 0o700)
                    retained = data / "retained"
                    retained.write_bytes(b"existing installation")
                    data.chmod(0o755)
                    result = shell(script, mask, data)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(data.stat().st_mode & 0o777, 0o755)
                    self.assertEqual(retained.read_bytes(), b"existing installation")

    def test_cleanup_and_publisher_reuse_installer_path_and_keep_overrides(self):
        publisher = INSTALLER.with_name("publish-release.sh").read_text()
        function = publisher[publisher.index("default_elastos_data_dir() {"):publisher.index("discover_source_bootstrap_json() {")]
        script = '''
cd "$1"
source scripts/lib/runtime-cleanup.sh
uname() { echo Darwin; }
HOME="/test/private home"
XDG_DATA_HOME=/test/xdg
[[ "$(elastos_runtime_data_dir "$HOME" "$XDG_DATA_HOME")" == "$HOME/Library/Application Support/elastos" ]]
''' + function + '''
[[ "$(default_elastos_data_dir)" == "$HOME/Library/Application Support/elastos" ]]
ELASTOS_DATA_DIR=/explicit/data
[[ "$(default_elastos_data_dir)" == /explicit/data ]]
ELASTOS_HOST_DATA_DIR=/explicit/host
[[ "$(default_elastos_data_dir)" == /explicit/host ]]
'''
        result = shell(script, INSTALLER.resolve().parent.parent)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_sourcing_defines_helpers_without_bootstrap_side_effects(self):
        result = subprocess.run([OPTIONS.bash, "--noprofile", "--norc", "-s", "--", str(INSTALLER.resolve())],
                                input='''
set +u
GATEWAYS=(kept)
MAINTAINER_DID=kept
options="$(set +o)"
source "$1"
[[ "$options" == "$(set +o)" && "$MAINTAINER_DID" == kept && "${GATEWAYS[0]}" == kept ]]
declare -F installer_data_dir >/dev/null
declare -F installer_runtime_control >/dev/null
''', text=True, capture_output=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(result.stdout, "")

    def test_stdin_entry_still_executes_help(self):
        result = subprocess.run([OPTIONS.bash, "--noprofile", "--norc", "-s", "--", "--help"],
                                input=SOURCE, text=True, capture_output=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("ElastOS Installer", result.stdout)


class NativeProcessTests(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory(prefix="elastos installer process ")
        self.root = Path(self.scratch.name)
        self.home = self.root / "home"
        self.binary = self.home / ".local/bin/elastos"
        self.binary.parent.mkdir(parents=True)
        # A private link to native cat blocks opening a FIFO named 'serve'.
        # Keep the system-signed binary intact on macOS, and exercise real
        # command/UID/start checks without starting Runtime.
        self.binary.symlink_to("/bin/cat")
        os.mkfifo(self.root / "serve", 0o600)
        self.child = subprocess.Popen([str(self.binary), "serve"], cwd=self.root,
                                      stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                      stderr=subprocess.DEVNULL)
        self.extra_children = []
        self.fixture_descendants = []
        time.sleep(0.1)
        self.assertIsNone(self.child.poll(), "Native FIFO fixture must remain alive before cleanup")
        self.data = self.home / ("Library/Application Support/elastos" if platform.system() == "Darwin" else "xdg-data/elastos")
        self.data.mkdir(parents=True)
        self.coords = self.data / "runtime-coords.json"
        self.write_coords(self.coords, self.child.pid)

    def tearDown(self):
        for child in [self.child, *self.extra_children]:
            if child.poll() is None:
                child.terminate()
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait(timeout=5)
        for pid, expected in self.fixture_descendants:
            if PROCESSES["process_snapshot"](pid) == expected:
                os.kill(pid, signal.SIGKILL)
        self.scratch.cleanup()

    def write_coords(self, path, pid):
        path.write_text(json.dumps({"pid": pid, "runtime_kind": "operator",
                                    "binary_sha256": hashlib.sha256(self.binary.read_bytes()).hexdigest()}))
        path.chmod(0o600)

    def start_descendant_fixture(self, stops_child):
        self.child.terminate()
        self.child.wait(timeout=5)
        self.binary.unlink()
        self.binary.symlink_to("/bin/sh")
        script = self.root / "serve"
        script.unlink()
        script.write_text('/bin/sleep 30 &\nchild=$!\n'
                          + ('trap \'kill "$child"; wait "$child"; exit 0\' TERM\n' if stops_child else '')
                          + 'printf "%s\\n" "$child"\nwait "$child"\n')
        self.child = subprocess.Popen([str(self.binary), "serve"], cwd=self.root,
                                      stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                      stderr=subprocess.DEVNULL, text=True)
        pid = int(self.child.stdout.readline())
        self.child.stdout.close()
        snapshot = PROCESSES["process_snapshot"](pid)
        self.assertIsNotNone(snapshot)
        self.fixture_descendants.append((pid, snapshot))
        self.write_coords(self.coords, self.child.pid)
        return pid, snapshot

    def test_real_process_liveness_and_same_hash_quiescence_before_install(self):
        snapshot = PROCESSES["process_snapshot"](self.child.pid)
        self.assertIsNotNone(snapshot)
        self.assertTrue(PROCESSES["matches_command"](snapshot, str(self.binary)))
        # Execute the actual pre-install block. It must stop the selected child
        # even though the selected binary hash is unchanged, before file writes.
        block = SOURCE.split("# ── Install (2 files)", 1)[1]
        block = block[block.index("\n"):block.index('info "Installing binary')]
        result = shell('HOME="$1"\nXDG_DATA_HOME="$1/xdg-data"\nINSTALL_DIR="$1/.local/bin"\nBINARY_SHA256="$2"\n' + block,
                       self.home, hashlib.sha256(self.binary.read_bytes()).hexdigest())
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.child.wait(timeout=5)
        self.assertIsNone(PROCESSES["process_snapshot"](self.child.pid))
        self.assertFalse(self.coords.exists())
        self.assertTrue(self.binary.is_file())

    def test_shared_wrapper_cleanup_finishes_before_home_removal(self):
        descendant, _ = self.start_descendant_fixture(stops_child=True)
        result = shell('''
source "$1/scripts/lib/runtime-cleanup.sh"
cleanup_elastos_runtime_home "$2" "$3" "$2/.local/bin/elastos"
rm -rf "$2"
''', INSTALLER.resolve().parent.parent, self.home, self.data)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.child.wait(timeout=5)
        self.assertIsNone(PROCESSES["process_snapshot"](descendant))
        self.assertFalse(self.home.exists())

    def test_surviving_descendant_preserves_wrapper_home(self):
        descendant, snapshot = self.start_descendant_fixture(stops_child=False)
        original = self.coords.read_bytes()
        source = INSTALLER.with_name("public-install-identity-smoke.sh").read_text()
        cleanup = source[source.index("cleanup() {"):source.index("trap cleanup EXIT")]
        result = shell('''
source "$1/scripts/lib/runtime-cleanup.sh"
HOME_DIR="$2" DATA_DIR="$3" RUN_BIN="$4"
''' + cleanup + '\ntrap cleanup EXIT\n', INSTALLER.resolve().parent.parent, self.home, self.data, self.binary)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("child remains active", result.stderr)
        self.assertIn("preserved", result.stderr)
        self.child.wait(timeout=5)
        self.assertEqual(PROCESSES["process_snapshot"](descendant), snapshot)
        self.assertEqual(self.coords.read_bytes(), original)
        self.assertTrue(self.home.is_dir())

    def test_changed_descendant_identity_preserves_state(self):
        snapshot = PROCESSES["process_snapshot"](self.child.pid)
        with self.assertRaisesRegex(ValueError, "child identity changed"):
            PROCESSES["wait_for_descendants"]({self.child.pid: ("changed start", snapshot[1])})
        self.assertIsNone(self.child.poll())
        self.assertTrue(self.coords.exists())

    def test_foreign_record_preserves_all_processes_and_state(self):
        foreign = subprocess.Popen(["/bin/sleep", "20"])
        self.extra_children.append(foreign)
        other = self.data / "home-runtime-coords.json"
        self.write_coords(other, foreign.pid)
        original = {path: path.read_bytes() for path in [self.coords, other]}
        with self.assertRaisesRegex(ValueError, "foreign or ambiguous"):
            PROCESSES["stop_installation"](str(self.data), str(self.binary), False)
        self.assertIsNone(self.child.poll())
        self.assertIsNone(foreign.poll())
        self.assertEqual({path: path.read_bytes() for path in original}, original)

    def test_start_identity_change_prevents_signals(self):
        snapshot = PROCESSES["process_snapshot"](self.child.pid)
        changed = ("different start", snapshot[1])
        real_kill = os.kill
        with mock.patch.object(os, "kill", wraps=real_kill) as signals:
            with self.assertRaisesRegex(ValueError, "identity changed"):
                PROCESSES["stop_owned_process"](self.child.pid, changed, str(self.binary))
            self.assertTrue(all(call.args[1] == 0 for call in signals.call_args_list))
        self.assertIsNone(self.child.poll())

    def test_stale_record_rejects_newer_process_before_any_signal(self):
        os.utime(self.coords, (0, 0))
        original = self.coords.read_bytes()
        real_kill = os.kill
        with mock.patch.object(os, "kill", wraps=real_kill) as signals:
            with self.assertRaisesRegex(ValueError, "started after its ownership record"):
                PROCESSES["stop_installation"](str(self.data), str(self.binary), True)
            self.assertTrue(all(call.args[1] == 0 for call in signals.call_args_list))
        self.assertIsNone(self.child.poll())
        self.assertEqual(self.coords.read_bytes(), original)
        self.assertEqual(self.coords.stat().st_mtime, 0)

    def test_owned_process_escalation_finishes_cleanup(self):
        child = subprocess.Popen([str(self.binary), "serve"], cwd=self.root,
                                 stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                 stderr=subprocess.DEVNULL,
                                 preexec_fn=lambda: signal.signal(signal.SIGTERM, signal.SIG_IGN))
        self.extra_children.append(child)
        snapshot = PROCESSES["process_snapshot"](child.pid)
        real_kill = os.kill
        with mock.patch.object(os, "kill", wraps=real_kill) as signals:
            PROCESSES["stop_owned_process"](child.pid, snapshot, str(self.binary))
            sent = [call.args[1] for call in signals.call_args_list]
            self.assertIn(signal.SIGTERM, sent)
            self.assertIn(signal.SIGKILL, sent)
        child.wait(timeout=5)

    def test_permission_and_ambiguous_observation_preserve_state(self):
        with mock.patch.object(os, "kill", side_effect=PermissionError):
            with self.assertRaisesRegex(ValueError, "another owner"):
                PROCESSES["process_snapshot"](self.child.pid)
        with mock.patch.object(subprocess, "run", return_value=subprocess.CompletedProcess([], 0, "incomplete", "")):
            with self.assertRaisesRegex(ValueError, "ambiguous"):
                PROCESSES["process_snapshot"](self.child.pid)
        foreign = f"{os.geteuid() + 1} Thu Sep 10 10:00:00 2026 S {self.binary} serve"
        with mock.patch.object(subprocess, "run", return_value=subprocess.CompletedProcess([], 0, foreign, "")):
            with self.assertRaisesRegex(ValueError, "another owner"):
                PROCESSES["process_snapshot"](self.child.pid)
        self.assertIsNone(self.child.poll())
        self.assertTrue(self.coords.exists())

    def test_invalid_pid_and_symlink_records_are_preserved(self):
        for pid in [0, 1, -1, True, "123", 2**31]:
            self.write_coords(self.coords, pid)
            original = self.coords.read_bytes()
            with self.subTest(pid=pid), self.assertRaises(ValueError):
                PROCESSES["stop_installation"](str(self.data), str(self.binary), False)
            self.assertEqual(self.coords.read_bytes(), original)
            self.assertIsNone(self.child.poll())
        self.coords.unlink()
        target = self.root / "foreign.json"
        self.write_coords(target, self.child.pid)
        self.coords.symlink_to(target)
        with self.assertRaises(OSError):
            PROCESSES["stop_installation"](str(self.data), str(self.binary), False)
        self.assertTrue(self.coords.is_symlink())
        self.assertIsNone(self.child.poll())

    def test_dead_and_zombie_processes_leave_no_stale_coords(self):
        self.child.terminate()
        # Poll ps without reaping first, so an exited child is a zombie.
        for _ in range(100):
            if PROCESSES["process_snapshot"](self.child.pid) is None:
                break
            time.sleep(0.01)
        self.assertIsNone(PROCESSES["process_snapshot"](self.child.pid))
        PROCESSES["stop_installation"](str(self.data), str(self.binary), False)
        self.assertFalse(self.coords.exists())
        self.child.wait(timeout=5)

    def test_branch_override_cleanup_preserves_other_home_process(self):
        other = subprocess.Popen([str(self.binary), "serve"], cwd=self.root,
                                 stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                 stderr=subprocess.DEVNULL)
        self.extra_children.append(other)
        result = shell('''
source "$1/scripts/lib/runtime-cleanup.sh"
cleanup_elastos_runtime_home "$2/override-home" "$3" "$4"
''', INSTALLER.resolve().parent.parent, self.home, self.data, self.binary)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.child.wait(timeout=5)
        self.assertIsNone(other.poll())
        self.assertFalse(self.coords.exists())

    def test_same_binary_other_data_root_aborts_before_any_signal(self):
        other = subprocess.Popen([str(self.binary), "serve"], cwd=self.root,
                                 stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                 stderr=subprocess.DEVNULL)
        self.extra_children.append(other)
        other_data = self.root / "another data root"
        other_data.mkdir()
        other_coords = other_data / "runtime-coords.json"
        self.write_coords(other_coords, other.pid)
        original = {path: path.read_bytes() for path in [self.coords, other_coords]}
        with self.assertRaisesRegex(ValueError, "no ownership record"):
            PROCESSES["stop_installation"](str(self.data), str(self.binary), True)
        self.assertIsNone(self.child.poll())
        self.assertIsNone(other.poll())
        self.assertEqual({path: path.read_bytes() for path in original}, original)

    def test_public_wrapper_cleanup_preserves_home_when_owner_is_ambiguous(self):
        foreign = subprocess.Popen(["/bin/sleep", "20"])
        self.extra_children.append(foreign)
        self.write_coords(self.coords, foreign.pid)
        original = self.coords.read_bytes()
        for name in ["public-install-identity-smoke.sh", "public-install-home-frontdoor-smoke.sh"]:
            source = INSTALLER.with_name(name).read_text()
            cleanup = source[source.index("cleanup() {"):source.index("trap cleanup EXIT")]
            result = shell('''
source "$1/scripts/lib/runtime-cleanup.sh"
HOME_DIR="$2" DATA_DIR="$3" RUN_BIN="$4"
''' + cleanup + '\ntrap cleanup EXIT\n', INSTALLER.resolve().parent.parent, self.home, self.data, self.binary)
            with self.subTest(wrapper=name):
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("preserved", result.stderr)
                self.assertTrue(self.home.is_dir())
                self.assertEqual(self.coords.read_bytes(), original)
                self.assertIsNone(foreign.poll())
                self.assertIsNone(self.child.poll())

    def test_changed_coords_are_preserved(self):
        self.child.terminate()
        self.child.wait(timeout=5)
        recorded = PROCESSES["read_coords"](self.coords)
        value = json.loads(self.coords.read_text())
        value["new_state"] = True
        self.coords.write_text(json.dumps(value))
        current = self.coords.read_bytes()
        with self.assertRaisesRegex(ValueError, "coordinates changed"):
            PROCESSES["remove_dead_coords"](self.coords, recorded)
        self.assertEqual(self.coords.read_bytes(), current)


class PublisherFixtureTests(unittest.TestCase):
    def setUp(self):
        if not OPTIONS.publisher_fixtures:
            self.skipTest("Pass --publisher-fixtures DIR to check retained public response bytes")
        self.fixtures = Path(OPTIONS.publisher_fixtures)

    def test_actual_signature_wrapper_accepts_both_signed_documents_without_openssl(self):
        for name, domain in [("release-head.json", "elastos.release.head.v1"), ("release.json", "elastos.release.v1")]:
            result = shell('openssl() { echo unexpected-openssl-call >&2; exit 92; }\nverify_signature "$1" "$2" "$3"\n',
                           self.fixtures / name, domain, PUBLISHER_DID)
            with self.subTest(document=name):
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn("Signature verified", result.stdout)

    def test_envelope_mutations_fail_closed(self):
        original = json.loads((self.fixtures / "release.json").read_text())
        cases = []
        for field, value in [("version", "tampered"), ("signer_did", "other")]:
            altered = copy.deepcopy(original)
            altered["payload"][field] = value
            cases.append(json.dumps(altered))
        for field, value in [("signer_did", "other"), ("signature", "00" * 64), ("signature", "0" * 127),
                             ("signature", "g" * 128), ("payload", None)]:
            altered = copy.deepcopy(original)
            altered[field] = value
            cases.append(json.dumps(altered))
        cases += ['{"signer_did":"duplicate",' + json.dumps(original)[1:], "{"]
        with tempfile.TemporaryDirectory(prefix="installer fixture's ") as directory:
            path = Path(directory, "release.json")
            for index, text in enumerate(cases):
                path.write_text(text)
                result = shell('verify_signature "$1" elastos.release.v1 "$2"\n', path, PUBLISHER_DID)
                with self.subTest(case=index):
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn("Signature verification FAILED", result.stderr)
                    self.assertNotIn("Signature verified", result.stdout)

    def test_wrong_domain_and_trust_anchor_fail_closed(self):
        path = self.fixtures / "release.json"
        for domain, did in [("elastos.release.head.v1", PUBLISHER_DID), ("elastos.release.v1", "other")]:
            result = shell('verify_signature "$1" "$2" "$3"\n', path, domain, did)
            self.assertNotEqual(result.returncode, 0)

    def test_unbound_signed_legacy_publication_is_rejected_before_artifacts(self):
        for transport in ["publisher", "cid"]:
            result, requests = run_offline_installer(
                (self.fixtures / "release-head.json").read_bytes(),
                (self.fixtures / "release.json").read_bytes(), PUBLISHER_DID, transport)
            with self.subTest(transport=transport):
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(result.stdout.count("Signature verified"), 2)
                self.assertIn("requires a lowercase SHA-256 envelope binding", result.stderr)
                self.assertEqual(len(requests), 2)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bash", default="/bin/bash")
    parser.add_argument("--publisher-fixtures")
    OPTIONS = parser.parse_args()
    unittest.main(argv=[__file__], verbosity=2)
