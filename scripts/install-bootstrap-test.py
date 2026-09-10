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
                paths[0].write_text(json.dumps(first))
                paths[1].write_text(json.dumps(second))
                result = shell('validate_release_identity "$1" "$2"\n', *paths)
                with self.subTest(case=index):
                    self.assertEqual(result.returncode == 0, accepted, result.stdout + result.stderr)


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

    def test_installer_checks_signatures_and_identity_before_artifact_request(self):
        # Execute the entire installer with only uname/curl replaced. curl reads
        # retained local bytes and rejects artifact requests. No install runs.
        with tempfile.TemporaryDirectory(prefix="installer-offline-") as directory:
            root = Path(directory)
            mockbin = root / "mocks"
            mockbin.mkdir()
            (mockbin / "uname").write_text('#!/bin/sh\ncase "$1" in -s) echo "${MOCK_SYSTEM:-Linux}";; -m) echo "${MOCK_MACHINE:-x86_64}";; esac\n')
            (mockbin / "curl").write_text('''#!/bin/sh
destination=""
while [ "$#" -gt 0 ]; do
    case "$1" in -o) destination="$2"; shift 2;; *) url="$1"; shift;; esac
done
printf '%s\\n' "$url" >> "$FIXTURES/requests"
case "$url" in
  */release-head.json) cp "$FIXTURES/release-head.json" "$destination";;
  */release.json) cp "$FIXTURES/release.json" "$destination";;
  *) echo artifact-request-blocked >&2; exit 93;;
esac
''')
            for path in mockbin.iterdir():
                path.chmod(0o755)
            first = json.loads((self.fixtures / "release-head.json").read_text())
            second = json.loads((self.fixtures / "release.json").read_text())
            for field, system, machine in [("version", "Linux", "x86_64"), ("channel", "Linux", "x86_64"),
                                           (None, "Linux", "x86_64"), (None, "Darwin", "arm64")]:
                altered = copy.deepcopy(second)
                if field:
                    altered["payload"][field] = "other"
                (root / "release-head.json").write_text(json.dumps(first))
                (root / "release.json").write_text(json.dumps(altered))
                (root / "requests").write_text("")
                # Mutations use --allow-unsigned to isolate the identity gate.
                # The matching case uses both real signed documents and the
                # pinned DID, then stops at the mock artifact request.
                result = shell('''
export HOME="$1/home" FIXTURES="$1" PATH="$1/mocks:$PATH"
export ELASTOS_PUBLISHER_GATEWAY=https://test.invalid
export MOCK_SYSTEM="$4" MOCK_MACHINE="$5"
exec "$2" --noprofile --norc "$3" "${@:6}"
''', root, OPTIONS.bash, INSTALLER, system, machine,
                    *(["--allow-unsigned"] if field else ["--maintainer-did", PUBLISHER_DID]))
                with self.subTest(field=field, system=system):
                    self.assertNotEqual(result.returncode, 0)
                    expected = ("Release identity check failed" if field else
                                "No release available for platform: aarch64-darwin" if system == "Darwin" else
                                "artifact-request-blocked")
                    self.assertIn(expected, result.stderr)
                    self.assertEqual((root / "requests").read_text().splitlines(),
                                     ["https://test.invalid/release-head.json", "https://test.invalid/release.json"]
                                     + ([] if field or system == "Darwin" else ["https://test.invalid/artifacts/elastos-x86_64-linux"]))
                    self.assertFalse((root / "home/.local/bin/elastos").exists())


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bash", default="/bin/bash")
    parser.add_argument("--publisher-fixtures")
    OPTIONS = parser.parse_args()
    unittest.main(argv=[__file__], verbosity=2)
