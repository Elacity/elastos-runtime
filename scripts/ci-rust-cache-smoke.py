#!/usr/bin/env python3
"""Prove Rust dependency cache reuse and source/release-stamp invalidation."""
import argparse
from contextlib import contextmanager
import hashlib
from http.server import BaseHTTPRequestHandler, HTTPServer
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
from threading import Thread
import time


@contextmanager
def read_only_gha(report):
    """Use sccache 0.18's GHA v2 RPC with a private, always-empty backend."""
    lookup = "/twirp/github.actions.results.api.v1.CacheService/GetCacheEntryDownloadURL"
    requests = []

    class Handler(BaseHTTPRequestHandler):
        def setup(self):
            self.request.settimeout(5)
            super().setup()

        def log_message(self, _format, *_args):
            pass

        def respond(self):
            requests.append((self.command, self.path))
            length = int(self.headers.get("Content-Length", "0"))
            valid = (self.command == "POST" and self.path == lookup
                     and self.headers.get("Content-Type") == "application/protobuf"
                     and self.headers.get("Authorization") == "Bearer ci-cache-probe"
                     and 0 < length <= 4096)
            if valid:
                valid = len(self.rfile.read(length)) == length
            # Locked opendal-service-ghac 0.58.1: bool ok is protobuf field 1.
            body = b"\x08\x00" if valid else b""
            self.send_response(200 if valid else 403)
            self.send_header("Content-Type", "application/protobuf")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            if not valid:
                report.setdefault("unexpected_remote_requests", []).append(self.path)

        do_POST = do_GET = do_PUT = do_PATCH = do_DELETE = do_HEAD = do_OPTIONS = respond

    server = HTTPServer(("127.0.0.1", 0), Handler)
    worker = Thread(target=server.serve_forever, daemon=True)
    worker.start()
    try:
        yield f"http://127.0.0.1:{server.server_port}/"
    finally:
        server.shutdown()
        worker.join(timeout=5)
        server.server_close()
        report["remote_lookup_requests"] = sum(item == ("POST", lookup) for item in requests)
        report["remote_write_requests"] = sum(item != ("POST", lookup) for item in requests)
        report["isolated_remote_stopped"] = not worker.is_alive()
        if worker.is_alive():
            raise RuntimeError("owned GHA fixture thread did not exit")
    if not report["remote_lookup_requests"] or report["remote_write_requests"] or report.get("unexpected_remote_requests"):
        raise RuntimeError("expected remote cache reads and zero remote writes")


def cleanup_group(child):
    def alive():
        child.poll()
        try:
            os.killpg(child.pid, 0)
            return True
        except ProcessLookupError:
            return False

    for sig, seconds in ((signal.SIGTERM, 2), (signal.SIGKILL, 5)):
        if not alive():
            break
        try:
            os.killpg(child.pid, sig)
        except ProcessLookupError:
            break
        deadline = time.monotonic() + seconds
        while alive() and time.monotonic() < deadline:
            child.poll()
            time.sleep(0.05)
    if alive():
        # Keep the caller's build lease until this owned group is gone, even
        # if exceptional kernel cleanup exceeds the normal command timeout.
        print(f"waiting for owned probe process group {child.pid} to exit", file=sys.stderr)
        while alive():
            child.poll()
            time.sleep(0.05)
    child.wait()


def run(command, root, env, timeout=120):
    child = subprocess.Popen(command, cwd=root, env=env, stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE, text=True, start_new_session=True)
    try:
        stdout, stderr = child.communicate(timeout=timeout)
        if child.returncode:
            raise RuntimeError(f"{Path(command[0]).name} failed: {stderr.strip()}")
        return stdout.strip()
    finally:
        cleanup_group(child)


def check_server(server):
    if server.poll() is not None:
        raise RuntimeError("owned sccache server exited unexpectedly")


def stats(sccache, root, env):
    # v0.18.0 serializes ServerInfo.stats and PerLanguageCount.counts;
    # adv_counts is a second view of the same counts, not another total.
    return json.loads(run([sccache, "--show-stats", "--stats-format=json"], root, env))["stats"]


def delta(before, after):
    result = {name: after[name] - before[name] for name in (
        "compile_requests", "requests_not_cacheable", "compilations",
        "compile_fails", "cache_writes", "cache_write_errors", "cache_read_errors")}
    for name in ("cache_hits", "cache_misses", "cache_errors"):
        result[name] = after[name]["counts"].get("Rust", 0) - before[name]["counts"].get("Rust", 0)
    return result


def write_fixture(root):
    for name in ("a", "b", "leaf", "component"):
        package = root / name
        (package / "src").mkdir(parents=True)
        manifest = f'[package]\nname = "cache_probe_{name}"\nversion = "0.1.0"\nedition = "2021"\n'
        if name in ("a", "b"):
            manifest += ('[dependencies]\ncache_probe_leaf = { path = "../leaf" }\n'
                         'cache_probe_component = { path = "../component" }\n'
                         '[profile.release]\nopt-level = 1\nlto = false\nincremental = false\n')
            source = 'fn main() { println!("{}|{}", cache_probe_leaf::value(), cache_probe_component::stamp()); }\n'
        elif name == "leaf":
            source = "pub fn value() -> u32 { 41 }\n"
        else:
            source = 'pub fn stamp() -> &\'static str { option_env!("ELASTOS_RELEASE_VERSION").unwrap_or("unset") }\n'
        (package / "Cargo.toml").write_text(manifest)
        (package / "src" / ("main.rs" if name in ("a", "b") else "lib.rs")).write_text(source)


def library_hashes(target):
    result = {}
    for name in ("leaf", "component"):
        paths = list((target / "release/deps").glob(f"libcache_probe_{name}-*.rlib"))
        if len(paths) != 1:
            raise RuntimeError(f"expected one {name} rlib, got {len(paths)}")
        result[name] = hashlib.sha256(paths[0].read_bytes()).hexdigest()
    return result


def probe(args, report):
    if os.name != "posix":
        raise RuntimeError("this probe uses a private POSIX Unix socket")
    tools = {}
    for name in ("sccache", "rustc", "cargo"):
        found = shutil.which(getattr(args, name, None) or name)
        if not found:
            raise RuntimeError(f"{name} executable is required")
        # Preserve Rustup proxy basenames instead of resolving their symlinks.
        tools[name] = os.path.abspath(found)
    with tempfile.TemporaryDirectory(prefix="ci-rust-cache-", dir="/tmp") as directory, read_only_gha(report) as gha_url:
        root = Path(directory)
        report["fixture_dir"] = str(root)
        target = root / "target"
        build = root / "build"
        uds = root / "server.sock"
        write_fixture(root)
        shutil.copyfile(Path(__file__).resolve().parents[1] / "rust-toolchain.toml",
                        root / "rust-toolchain.toml")
        (root / "sccache.json").write_text("{}\n")
        env = {k: v for k, v in os.environ.items()
               if not k.startswith(("SCCACHE_", "CARGO_")) and k != "OPENDAL_TEST"}
        env.update(SCCACHE_DIR=str(root / "cache"), SCCACHE_CACHE_SIZE="64M",
                   SCCACHE_CONF=str(root / "sccache.json"),
                   SCCACHE_CACHED_CONF=str(root / "sccache-cached.toml"),
                   SCCACHE_SERVER_UDS=str(uds), SCCACHE_GHA_ENABLED="on",
                   SCCACHE_MULTILEVEL_CHAIN="disk,gha", SCCACHE_LOCAL_RW_MODE="READ_WRITE",
                   SCCACHE_GHA_RW_MODE="READ_ONLY", SCCACHE_MULTILEVEL_WRITE_ERROR_POLICY="l0",
                   ACTIONS_CACHE_SERVICE_V2="1", GITHUB_SERVER_URL="https://github.com",
                   ACTIONS_RESULTS_URL=gha_url, ACTIONS_CACHE_URL=gha_url,
                   ACTIONS_RUNTIME_TOKEN="ci-cache-probe", NO_PROXY="127.0.0.1", no_proxy="127.0.0.1",
                   SCCACHE_NO_DAEMON="1",
                   SCCACHE_IDLE_TIMEOUT="60", CARGO_HOME=str(root / "cargo-home"),
                    CARGO_TARGET_DIR=str(target), CARGO_BUILD_BUILD_DIR=str(build), CARGO_INCREMENTAL="0",
                   RUSTC=tools["rustc"], RUSTC_WRAPPER=tools["sccache"],
                   RUSTC_WORKSPACE_WRAPPER="", RUSTFLAGS="", CARGO_ENCODED_RUSTFLAGS="",
                   RUSTUP_TOOLCHAIN="1.91.0", ELASTOS_RELEASE_VERSION="probe-one")
        report["versions"] = {name: run([binary, "--version"], root, env)
                              for name, binary in tools.items()}
        for name, expected in (("sccache", "0.18.0"), ("rustc", "1.91.0"), ("cargo", "1.91.0")):
            if report["versions"][name].split()[:2] != [name, expected]:
                raise RuntimeError(f"expected {name} {expected}, got {report['versions'][name]}")
        report["target_dir"] = str(target)
        report["stages"] = []
        # Own the foreground server PID. v0.18.0 cmdline.rs accepts the internal
        # start marker; documented SCCACHE_NO_DAEMON keeps that PID foreground.
        # https://github.com/mozilla/sccache/blob/v0.18.0/src/cmdline.rs
        server_env = dict(env, SCCACHE_START_SERVER="1")
        with (root / "server.log").open("w") as log:
            server = subprocess.Popen([tools["sccache"]], cwd=root, env=server_env,
                                      stdin=subprocess.DEVNULL, stdout=log, stderr=log,
                                      start_new_session=True)
            try:
                deadline = time.monotonic() + 10
                while True:
                    if server.poll() is not None:
                        raise RuntimeError("private sccache server exited during startup")
                    try:
                        with socket.socket(socket.AF_UNIX) as connection:
                            connection.connect(str(uds))
                        break
                    except (FileNotFoundError, ConnectionRefusedError):
                        if time.monotonic() >= deadline:
                            raise RuntimeError("private sccache server startup timed out")
                        time.sleep(0.05)
                for package in ("a", "b"):
                    check_server(server)
                    run([tools["cargo"], "generate-lockfile", "--offline"], root / package, env)
                    check_server(server)
                previous_hashes = None
                stages = (("cold_a", "a", 41, "probe-one", 0, 2),
                          ("warm_a", "a", 41, "probe-one", 2, 0),
                          ("cold_b_shared_libraries", "b", 41, "probe-one", 2, 0),
                          ("source_change", "b", 42, "probe-one", 1, 1),
                          ("release_stamp_change", "b", 42, "probe-two", 1, 1),
                          ("repeat_changed", "b", 42, "probe-two", 2, 0),
                          ("output_path_change", "b", 42, "probe-two", 0, 2),
                          ("repeat_changed_output_path", "b", 42, "probe-two", 2, 0))
                for name, package, value, stamp, hits, misses in stages:
                    for output in (target, build):
                        if output.exists():
                            shutil.rmtree(output)
                    # sccache 0.18 hashes Cargo directory environment values.
                    # Fresh contents at fixed paths can reuse dependencies.
                    if name == "output_path_change":
                        target = root / "changed-target"
                        build = root / "changed-build"
                    env["CARGO_TARGET_DIR"] = str(target)
                    env["CARGO_BUILD_BUILD_DIR"] = str(build)
                    if target.exists() or build.exists():
                        raise RuntimeError(f"{name}: expected fresh build directories")
                    (root / "leaf/src/lib.rs").write_text(f"pub fn value() -> u32 {{ {value} }}\n")
                    env["ELASTOS_RELEASE_VERSION"] = stamp
                    check_server(server)
                    before = stats(tools["sccache"], root, env)
                    check_server(server)
                    start = time.monotonic()
                    run([tools["cargo"], "build", "--offline", "--locked", "--release",
                         "--jobs=1"], root / package, env)
                    seconds = time.monotonic() - start
                    check_server(server)
                    after = stats(tools["sccache"], root, env)
                    check_server(server)
                    change = delta(before, after)
                    hashes = library_hashes(build)
                    output = run([str(target / "release" / f"cache_probe_{package}")], root, env)
                    stage = dict(name=name, seconds=seconds, delta=change,
                                 target_dir=str(target), build_dir=str(build),
                                 library_sha256=hashes, executable_output=output,
                                 stats_before=before, stats_after=after)
                    report["stages"].append(stage)
                    if (change["cache_hits"], change["cache_misses"]) != (hits, misses):
                        raise RuntimeError(f"{name}: expected {hits} library hits/{misses} misses, got {change}")
                    if change["cache_writes"] != misses:
                        raise RuntimeError(f"{name}: expected {misses} local cache writes, got {change}")
                    if change["requests_not_cacheable"] < 1:
                        raise RuntimeError(f"{name}: expected the binary link to remain uncached")
                    if any(change[key] for key in ("cache_errors", "cache_write_errors", "cache_read_errors", "compile_fails")):
                        raise RuntimeError(f"{name}: cache/compiler errors: {change}")
                    if output != f"{value}|{stamp}":
                        raise RuntimeError(f"{name}: linked executable returned {output!r}")
                    if previous_hashes is not None:
                        changed = "leaf" if name == "source_change" else "component" if name == "release_stamp_change" else None
                        for library in hashes:
                            if (hashes[library] != previous_hashes[library]) != (library == changed):
                                raise RuntimeError(f"{name}: unexpected {library} rlib hash change")
                    previous_hashes = hashes
                fields = ("name", "hits", "misses", "writes", "write_failures")
                report["cache_levels"] = [dict(zip(fields, (level[field] for field in fields)))
                                           for level in after["multi_level"]]
                expected_levels = [dict(zip(fields, values)) for values in (
                    ("L0 (disk)", 10, 6, 6, 0), ("L1 (ghac)", 0, 6, 0, 0))]
                if report["cache_levels"] != expected_levels:
                    raise RuntimeError("expected writable disk reuse beside read-only GHA")
            finally:
                try:
                    run([tools["sccache"], "--stop-server"], root, env, timeout=10)
                    server.wait(timeout=10)
                except (RuntimeError, subprocess.SubprocessError):
                    pass  # The owned group cleanup also handles an exited server.
                finally:
                    cleanup_group(server)
                report["isolated_server_stopped"] = True
    report["temporary_fixture_removed"] = not root.exists()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sccache", help="sccache 0.18.0 executable (default: PATH)")
    parser.add_argument("--rustc", help="Rust 1.91.0 compiler (default: PATH)")
    parser.add_argument("--output-json", type=Path, help="also write the result JSON to this path")
    args = parser.parse_args()
    report = {"ok": False}
    def stop(_signal, _frame):
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        raise RuntimeError("probe received SIGTERM")
    previous_handler = signal.signal(signal.SIGTERM, stop)
    try:
        probe(args, report)
        report["ok"] = True
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        report["error"] = str(error)
    finally:
        signal.signal(signal.SIGTERM, previous_handler)
        if "fixture_dir" in report:
            report["temporary_fixture_removed"] = not Path(report["fixture_dir"]).exists()
    encoded = json.dumps(report, indent=2) + "\n"
    if args.output_json:
        args.output_json.write_text(encoded)
    print(encoded, end="")
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
