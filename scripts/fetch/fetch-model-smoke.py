#!/usr/bin/env python3
import hashlib
import http.server
import io
import json
import os
import stat
import subprocess
import tarfile
import tempfile
import threading
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
FETCH = ROOT / "scripts/fetch/fetch-model.sh"


def sha256(value):
    return "sha256:" + hashlib.sha256(value).hexdigest()


def engine_archive():
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w:gz") as archive:
        payload = b"fixture llama-server\n"
        dylib_payload = b"fixture dylib\n"
        directory = tarfile.TarInfo("llama-fixture-v1")
        directory.type = tarfile.DIRTYPE
        directory.mode = 0o755
        archive.addfile(directory)
        binary = tarfile.TarInfo("llama-fixture-v1/llama-server")
        binary.mode = 0o755
        binary.size = len(payload)
        archive.addfile(binary, io.BytesIO(payload))
        dylib = tarfile.TarInfo("llama-fixture-v1/libfixture.dylib")
        dylib.mode = 0o644
        dylib.size = len(dylib_payload)
        archive.addfile(dylib, io.BytesIO(dylib_payload))
        dylib_link = tarfile.TarInfo("llama-fixture-v1/libllama.dylib")
        dylib_link.type = tarfile.SYMTYPE
        dylib_link.linkname = "libfixture.dylib"
        archive.addfile(dylib_link)
    return output.getvalue()


class ArtifactServer:
    def __init__(self, artifacts):
        self.artifacts = artifacts
        self.requests = []
        owner = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                owner.requests.append(self.path)
                if self.path == "/interrupted.gguf":
                    payload = owner.artifacts[self.path]
                    self.send_response(200)
                    self.send_header("Content-Length", str(len(payload) + 64))
                    self.end_headers()
                    self.wfile.write(payload)
                    self.wfile.flush()
                    self.connection.shutdown(1)
                    return
                payload = owner.artifacts.get(self.path)
                if payload is None:
                    self.send_error(404)
                    return
                self.send_response(200)
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)

            def log_message(self, _format, *_args):
                pass

        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *_args):
        self.server.shutdown()
        self.thread.join()
        self.server.server_close()

    @property
    def origin(self):
        return f"http://127.0.0.1:{self.server.server_port}"


def write_manifest(path, origin, engine, stable_url="/stable.gguf", stable_checksum=None):
    path.parent.mkdir(parents=True, exist_ok=True)
    stable = b"stable model\n"
    bonsai = b"experimental model\n"
    manifest = {
        "external": {
            "llama-server": {
                "version": "fixture-v1",
                "platforms": {
                    "darwin-arm64": {
                        "url": origin + "/llama.tar.gz",
                        "checksum": sha256(engine),
                        "extract_path": "llama-fixture-v1",
                        "install_path": "libexec/llama.cpp/fixture-v1/darwin-arm64",
                        "binary_path": "llama-server",
                    },
                    "linux-amd64": {
                        "url": origin + "/llama.tar.gz",
                        "checksum": sha256(engine),
                        "extract_path": "llama-fixture-v1/llama-server",
                        "install_path": "bin/llama-server",
                    }
                },
            },
            "model-qwen3.5-9b": {
                "description": "stable fixture",
                "platforms": {
                    "*": {
                        "url": origin + stable_url,
                        "checksum": stable_checksum or sha256(stable),
                        "install_path": "models/stable.gguf",
                    }
                },
            },
            "model-bonsai-8b-q1": {
                "description": "experimental fixture",
                "platforms": {
                    "*": {
                        "url": origin + "/experimental.gguf",
                        "checksum": sha256(bonsai),
                        "install_path": "models/experimental.gguf",
                    }
                },
            },
        }
    }
    path.write_text(json.dumps(manifest), encoding="utf-8")
    return stable, bonsai, engine


def fixture_env(root, manifest, data, system="Darwin", machine="arm64"):
    tool_dir = root / "tools"
    tool_dir.mkdir(parents=True, mode=0o700)
    uname = tool_dir / "uname"
    uname.write_text(
        f'#!/usr/bin/env bash\n[[ "$1" == "-s" ]] && echo {system} || echo {machine}\n',
        encoding="utf-8",
    )
    uname.chmod(0o700)
    return {
        **os.environ,
        "PATH": f"{tool_dir}:{os.environ['PATH']}",
        "ELASTOS_COMPONENTS_MANIFEST": str(manifest),
        "ELASTOS_DATA_DIR": str(data),
        "ELASTOS_MODEL_ALLOW_LOCAL_HTTP_FIXTURE": "1",
    }


def run_fetch(env, candidate, expected=0):
    result = subprocess.run(
        ["bash", str(FETCH), candidate],
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        check=False,
    )
    if result.returncode != expected:
        raise AssertionError(
            f"fetch returned {result.returncode}, expected {expected}: "
            f"stdout={result.stdout!r} stderr={result.stderr!r}"
        )
    return result


def assert_reported_engine(result, expected):
    lines = [
        line for line in result.stdout.splitlines() if line.startswith("  llama-server: ")
    ]
    if lines != [f"  llama-server: {expected}"]:
        raise AssertionError(f"unexpected reported llama-server path: {lines}")


def assert_no_partial_downloads(data):
    leftovers = [path for path in data.rglob("*") if ".download." in path.name]
    if leftovers:
        raise AssertionError(f"partial downloads remain: {leftovers}")


def make_tree_removable(root):
    for directory, _, _ in os.walk(root):
        Path(directory).chmod(0o700)


def bundle_snapshot(root):
    snapshot = []

    def visit(directory):
        for child in sorted(os.scandir(directory), key=lambda entry: entry.name):
            metadata = child.stat(follow_symlinks=False)
            relative = Path(child.path).relative_to(root).as_posix()
            identity = (
                relative,
                metadata.st_ino,
                metadata.st_nlink,
                stat.S_IMODE(metadata.st_mode),
            )
            if stat.S_ISDIR(metadata.st_mode):
                snapshot.append(identity + ("directory",))
                visit(child.path)
            elif stat.S_ISREG(metadata.st_mode):
                snapshot.append(identity + ("file", Path(child.path).read_bytes()))
            elif stat.S_ISLNK(metadata.st_mode):
                snapshot.append(identity + ("symlink", os.readlink(child.path)))
            else:
                snapshot.append(identity + ("special",))

    visit(root)
    return snapshot


def install_mac_bundle(root, server):
    manifest = root / "components.json"
    data = root / "data"
    write_manifest(manifest, server.origin, server.artifacts["/llama.tar.gz"])
    env = fixture_env(root, manifest, data)
    start = len(server.requests)
    run_fetch(env, "stable")
    if server.requests[start:] != ["/stable.gguf", "/llama.tar.gz"]:
        raise AssertionError("macOS fixture did not fetch its initial artifacts")
    bundle = data / "libexec/llama.cpp/fixture-v1/darwin-arm64"
    return env, data, bundle


def assert_invalid_bundle_rejected(env, data, bundle, server):
    before = bundle_snapshot(bundle)
    start = len(server.requests)
    result = run_fetch(env, "stable", expected=1)
    if server.requests[start:]:
        raise AssertionError("invalid bundle triggered an artifact download")
    if "keep it for operator review" not in result.stderr:
        raise AssertionError("invalid bundle failure did not preserve operator evidence")
    if bundle_snapshot(bundle) != before:
        raise AssertionError("invalid bundle changed after failed reuse validation")
    assert_no_partial_downloads(data)


def test_platform_selection_and_verified_reuse(root, server):
    manifest = root / "components.json"
    data = root / "stable-data"
    engine = server.artifacts["/llama.tar.gz"]
    stable, _, _ = write_manifest(manifest, server.origin, engine)
    env = fixture_env(root, manifest, data)

    first = run_fetch(env, "stable")
    if "Platform:  darwin-arm64" not in first.stdout:
        raise AssertionError("macOS arm64 platform was not selected")
    if server.requests != ["/stable.gguf", "/llama.tar.gz"]:
        raise AssertionError(f"unexpected stable artifact requests: {server.requests}")
    model = data / "models/stable.gguf"
    archive = data / "artifacts/llama-fixture-v1-darwin-arm64.tar.gz"
    bundle = data / "libexec/llama.cpp/fixture-v1/darwin-arm64"
    binary = bundle / "llama-server"
    dylib = bundle / "libfixture.dylib"
    dylib_link = bundle / "libllama.dylib"
    receipt = bundle / ".elastos-engine.json"
    link = data / "bin/llama-server"
    assert_reported_engine(first, binary.resolve())
    if model.read_bytes() != stable or archive.read_bytes() != engine:
        raise AssertionError("verified artifacts were not installed exactly")
    if dylib.read_bytes() != b"fixture dylib\n":
        raise AssertionError("macOS dylib was not installed exactly")
    if not dylib_link.is_symlink() or os.readlink(dylib_link) != "libfixture.dylib":
        raise AssertionError("macOS dylib symlink was not installed exactly")
    if not link.is_symlink() or link.resolve() != binary.resolve():
        raise AssertionError(
            f"stable llama-server link is invalid: symlink={link.is_symlink()} "
            f"target={os.readlink(link) if link.is_symlink() else None} "
            f"resolved={link.resolve()} expected={binary.resolve()}"
        )
    expected_receipt = {
        "schema": "elastos.local-model-engine/v2",
        "version": "fixture-v1",
        "platform": "darwin-arm64",
        "archive_sha256": sha256(engine),
        "entries": [
            {
                "path": "libfixture.dylib",
                "sha256": sha256(b"fixture dylib\n"),
                "type": "file",
            },
            {
                "path": "libllama.dylib",
                "target": "libfixture.dylib",
                "type": "symlink",
            },
            {
                "path": "llama-server",
                "sha256": sha256(b"fixture llama-server\n"),
                "type": "file",
            },
        ],
    }
    if json.loads(receipt.read_text(encoding="utf-8")) != expected_receipt:
        raise AssertionError("macOS bundle receipt does not contain the exact inventory")
    expected_modes = {
        bundle: 0o500,
        binary: 0o500,
        dylib: 0o400,
        receipt: 0o400,
    }
    for path, expected_mode in expected_modes.items():
        actual_mode = stat.S_IMODE(path.stat().st_mode)
        if actual_mode != expected_mode:
            raise AssertionError(
                f"unexpected protected mode for {path.name}: {actual_mode:o}"
            )
    before = (
        model.stat().st_ino,
        archive.stat().st_ino,
        binary.stat().st_ino,
        dylib.stat().st_ino,
        dylib_link.lstat().st_ino,
        receipt.stat().st_ino,
    )

    second = run_fetch(env, "stable")
    after = (
        model.stat().st_ino,
        archive.stat().st_ino,
        binary.stat().st_ino,
        dylib.stat().st_ino,
        dylib_link.lstat().st_ino,
        receipt.stat().st_ino,
    )
    if server.requests != ["/stable.gguf", "/llama.tar.gz"] or before != after:
        raise AssertionError("verified reuse downloaded or replaced an artifact")
    if "already present and verified" not in second.stdout:
        raise AssertionError("verified reuse was not reported")
    assert_no_partial_downloads(data)


def test_changed_dylib_reuse_is_rejected(root, server):
    env, data, bundle = install_mac_bundle(root, server)
    dylib = bundle / "libfixture.dylib"
    dylib.chmod(0o600)
    dylib.write_bytes(b"changed dylib\n")
    dylib.chmod(0o400)
    assert_invalid_bundle_rejected(env, data, bundle, server)


def test_missing_bundle_file_reuse_is_rejected(root, server):
    env, data, bundle = install_mac_bundle(root, server)
    bundle.chmod(0o700)
    (bundle / "libfixture.dylib").unlink()
    bundle.chmod(0o500)
    assert_invalid_bundle_rejected(env, data, bundle, server)


def test_added_bundle_file_reuse_is_rejected(root, server):
    env, data, bundle = install_mac_bundle(root, server)
    bundle.chmod(0o700)
    added = bundle / "unexpected.dylib"
    added.write_bytes(b"unexpected dylib\n")
    added.chmod(0o400)
    bundle.chmod(0o500)
    assert_invalid_bundle_rejected(env, data, bundle, server)


def test_unsafe_bundle_symlink_reuse_is_rejected(root, server):
    for name, target in (
        ("absolute", "/private/tmp/outside.dylib"),
        ("escaping", "../outside.dylib"),
    ):
        env, data, bundle = install_mac_bundle(root / name, server)
        dylib_link = bundle / "libllama.dylib"
        bundle.chmod(0o700)
        dylib_link.unlink()
        dylib_link.symlink_to(target)
        bundle.chmod(0o500)
        assert_invalid_bundle_rejected(env, data, bundle, server)


def test_redirected_bundle_symlink_reuse_is_rejected(root, server):
    env, data, bundle = install_mac_bundle(root, server)
    dylib_link = bundle / "libllama.dylib"
    bundle.chmod(0o700)
    dylib_link.unlink()
    dylib_link.symlink_to("llama-server")
    bundle.chmod(0o500)
    assert_invalid_bundle_rejected(env, data, bundle, server)


def test_bundle_protection_reuse_is_rejected(root, server):
    for name, mutate in (
        ("directory-mode", lambda case_root, bundle: bundle.chmod(0o700)),
        (
            "file-mode",
            lambda case_root, bundle: (bundle / "libfixture.dylib").chmod(0o600),
        ),
        (
            "hardlink",
            lambda case_root, bundle: os.link(
                bundle / "libfixture.dylib", case_root / "outside-hardlink.dylib"
            ),
        ),
    ):
        case_root = root / name
        env, data, bundle = install_mac_bundle(case_root, server)
        mutate(case_root, bundle)
        assert_invalid_bundle_rejected(env, data, bundle, server)


def test_experimental_selection(root, server):
    manifest = root / "experimental-components.json"
    data = root / "experimental-data"
    _, bonsai, _ = write_manifest(
        manifest, server.origin, server.artifacts["/llama.tar.gz"]
    )
    env = fixture_env(root / "experimental-env", manifest, data)
    start = len(server.requests)
    run_fetch(env, "experimental")
    if server.requests[start:] != ["/experimental.gguf", "/llama.tar.gz"]:
        raise AssertionError("experimental candidate did not select its exact artifacts")
    if (data / "models/experimental.gguf").read_bytes() != bonsai:
        raise AssertionError("experimental model bytes differ")


def test_linux_file_install_shape(root, server):
    manifest = root / "linux-components.json"
    data = root / "linux-data"
    stable, _, _ = write_manifest(
        manifest, server.origin, server.artifacts["/llama.tar.gz"]
    )
    env = fixture_env(root / "linux-env", manifest, data, "Linux", "x86_64")
    start = len(server.requests)
    first = run_fetch(env, "stable")
    if "Platform:  linux-amd64" not in first.stdout:
        raise AssertionError("Linux amd64 platform was not selected")
    if server.requests[start:] != ["/stable.gguf", "/llama.tar.gz"]:
        raise AssertionError("Linux candidate did not select its exact artifacts")
    model = data / "models/stable.gguf"
    binary = data / "bin/llama-server"
    assert_reported_engine(first, binary.resolve())
    if model.read_bytes() != stable or binary.read_bytes() != b"fixture llama-server\n":
        raise AssertionError("Linux file-valued artifacts differ")
    if binary.is_symlink():
        raise AssertionError("Linux file-valued install became a bundle link")
    before = (model.stat().st_ino, binary.stat().st_ino)
    run_fetch(env, "stable")
    if server.requests[start:] != ["/stable.gguf", "/llama.tar.gz"]:
        raise AssertionError("verified Linux reuse downloaded an artifact")
    if before != (model.stat().st_ino, binary.stat().st_ino):
        raise AssertionError("verified Linux reuse replaced an artifact")


def test_checksum_rejection_preserves_existing(root, server):
    manifest = root / "bad-components.json"
    data = root / "bad-data"
    stable, _, _ = write_manifest(
        manifest,
        server.origin,
        server.artifacts["/llama.tar.gz"],
        stable_checksum=sha256(b"expected different bytes"),
    )
    existing = data / "models/stable.gguf"
    existing.parent.mkdir(parents=True)
    existing.write_bytes(b"existing model\n")
    inode = existing.stat().st_ino
    env = fixture_env(root / "bad-env", manifest, data)
    result = run_fetch(env, "stable", expected=1)
    if "was not installed" not in result.stderr:
        raise AssertionError("checksum failure was not explicit")
    if existing.read_bytes() != b"existing model\n" or existing.stat().st_ino != inode:
        raise AssertionError("checksum failure replaced the existing model")
    if stable == existing.read_bytes():
        raise AssertionError("checksum fixture is invalid")
    assert_no_partial_downloads(data)


def test_interrupted_download_cleanup(root, server):
    manifest = root / "interrupted-components.json"
    data = root / "interrupted-data"
    stable, _, _ = write_manifest(
        manifest,
        server.origin,
        server.artifacts["/llama.tar.gz"],
        stable_url="/interrupted.gguf",
    )
    server.artifacts["/interrupted.gguf"] = stable
    env = fixture_env(root / "interrupted-env", manifest, data)
    result = run_fetch(env, "stable", expected=1)
    if "Download failed" not in result.stderr:
        raise AssertionError("interrupted download did not fail clearly")
    if (data / "models/stable.gguf").exists():
        raise AssertionError("interrupted download published a model")
    assert_no_partial_downloads(data)


def main():
    artifacts = {
        "/stable.gguf": b"stable model\n",
        "/experimental.gguf": b"experimental model\n",
        "/llama.tar.gz": engine_archive(),
    }
    with tempfile.TemporaryDirectory(prefix="elastos-model-bootstrap-") as temp:
        root = Path(temp)
        try:
            with ArtifactServer(artifacts) as server:
                test_platform_selection_and_verified_reuse(root / "reuse", server)
                test_changed_dylib_reuse_is_rejected(root / "changed-dylib", server)
                test_missing_bundle_file_reuse_is_rejected(root / "missing-file", server)
                test_added_bundle_file_reuse_is_rejected(root / "added-file", server)
                test_unsafe_bundle_symlink_reuse_is_rejected(
                    root / "unsafe-link", server
                )
                test_redirected_bundle_symlink_reuse_is_rejected(
                    root / "redirected-link", server
                )
                test_bundle_protection_reuse_is_rejected(root / "protection", server)
                test_experimental_selection(root / "experimental", server)
                test_linux_file_install_shape(root / "linux", server)
                test_checksum_rejection_preserves_existing(root / "checksum", server)
                test_interrupted_download_cleanup(root / "interrupted", server)
        finally:
            make_tree_removable(root)
    print("PASS local model bootstrap smoke")


if __name__ == "__main__":
    main()
