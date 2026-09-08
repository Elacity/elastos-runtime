#!/usr/bin/env python3
"""Observe UP-00 with an explicitly approved, prepared fixture. Stdlib only.

  update-hop-harness.sh inspect fixture.json
  update-hop-harness.sh run fixture.json /stable/path/new-receipt-directory
  update-hop-compare.py compare observation.json

The fixture JSON has schema `elastos.update-hop.fixture/v1`, an `approval`
reference, `proof_kind` (`real-runtime` or `harness-self-test`), `root`, `source`
{commit, tree}, `old` and `new` {version, binary_sha256, components_sha256,
source: {commit, tree}},
`channel`, and `ports` {publisher, cli, operator}. All four prepared homes are
root/homes/{publisher,controller,cli,operator}; each installs .local/bin/elastos.
The native data path is HOME/Library/Application Support/elastos on Mac and
HOME/xdg-data/elastos on Linux (Linux is permitted only for harness self-tests).
`preserve` maps cli/operator to {identity, chat, draft, library, model_stub}
paths relative to that Home's data directory. Each path must exist and contain
fixture data only. `catalogue` is a data-relative path; `new.catalogue_sha256`
binds the desired catalogue. `publication` supplies SHA-256 values for `head`
and `release`. The publisher has these signed envelopes and native artifacts
under ElastOS/SystemServices/Publisher, plus its authorized fixture identity.

Each Home already has its approved 32-byte identity/device.key. The supplied
old binaries all have the old hash. Both consumers have the old
components and source version. Their source signer matches the publisher DID.
Preparing/signing/installing fixtures belongs to the caller's explicit approval.
This harness neither builds nor signs. It runs one CLI and one operator apply
on separate old fixtures, keeps fixture data, and stops its own process groups.
Existing installed Homes, providers, Namespace and public discovery are outside
this bounded fixture. Use isolated operator-profile fixtures with no providers.
Both component sets have empty `external` maps; external asset replacement is
outside this baseline. Capsule names must be plain names inside the fixture.
Stage, Restart and Undo remain unsupported. A model stub proves bytes only.
"""

import argparse
import datetime
import fcntl
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import signal
import socket
import subprocess
import sys
import time
import urllib.request


def digest(path):
    h = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def json_digest(path):
    return hashlib.sha256(json.dumps(read(path), sort_keys=True, separators=(",", ":")).encode()).hexdigest()


def read(path):
    return json.loads(Path(path).read_text())


def write(path, value):
    Path(path).write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def need(condition, message):
    if not condition:
        raise ValueError(message)


def inside(root, relative):
    path = root / relative
    need(not Path(relative).is_absolute(), "fixture paths must be relative")
    need(path.resolve().is_relative_to(root.resolve()), "path escapes fixture root")
    need(not any(p.is_symlink() for p in [path, *path.parents]), "fixture path uses a symlink")
    return path


def home(config, role):
    return inside(Path(config["root"]), "homes/" + role)


def data(config, role):
    suffix = "Library/Application Support/elastos" if sys.platform == "darwin" else "xdg-data/elastos"
    return inside(home(config, role), suffix)


def binary(config, role):
    return inside(home(config, role), ".local/bin/elastos")


def environment(config, role):
    # A small environment prevents ambient relay, provider and source overrides.
    return {"PATH": "/usr/bin:/bin:/usr/sbin:/sbin",
            "HOME": str(home(config, role)), "XDG_DATA_HOME": str(home(config, role) / "xdg-data"),
            "ELASTOS_CARRIER_NETWORK": "direct", "ELASTOS_QUIET_RUNTIME_NOTICES": "1"}


def source(config, role):
    value = read(data(config, role) / "sources.json")
    return next(s for s in value["sources"] if s["name"] == value["default_source"])


def files(path):
    if not path.exists():
        return None
    paths = [path] if path.is_file() else sorted(path.rglob("*"))
    need(all(not p.is_symlink() for p in paths), "snapshot contains a symlink")
    return {str(p.relative_to(path)) if p != path else ".": digest(p)
            for p in paths if p.is_file()}


def fixture_components(path):
    manifest = read(path)
    need(manifest.get("external") == {}, "bounded fixture requires an empty external component map")
    need(all(re.fullmatch(r"[a-zA-Z0-9_-]+", name) for name in manifest.get("capsules", {})), "capsule name escapes the fixture cache")


def lock_state(path):
    if not path.exists():
        return "absent"
    with path.open("rb") as stream:
        try:
            fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return "held"
        fcntl.flock(stream, fcntl.LOCK_UN)
    return "released"


def health(port):
    try:
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        with opener.open(f"http://127.0.0.1:{port}/healthz", timeout=1) as response:
            return response.status == 200
    except (OSError, ValueError):
        return False


def operator_response_lost(output, host_exit):
    return host_exit == 75 and (
        "operator peer returned an empty response" in output
        or bool(re.search(r"Error: connection lost\s+Caused by:\s+timed out", output)))


def update_evidence(config, before, after, host_text, command_text):
    expected_hash = config["new"]["binary_sha256"]
    replaced = after["binary_sha256"] == expected_hash and before["binary_sha256"] != expected_hash
    install_line = (r"^[ \t]*Installing " + re.escape(config["old"]["version"])
                    + " → " + re.escape(config["new"]["version"]) + r"\.\.\.[ \t]*$")
    cache_line = (r"^[ \t]*(?:Capsule cache unchanged|Cleared [1-9][0-9]* changed cached capsule\(s\): "
                  r"[a-zA-Z0-9_-]+(?:, [a-zA-Z0-9_-]+)*)[ \t]*$")
    return {
        "attempted": replaced or bool(re.search(install_line, host_text + "\n" + command_text, re.M)),
        "cache_stage_observed": bool(re.search(cache_line, host_text, re.M)),
        "operator_response_lost": operator_response_lost(command_text, after["host_exit"]),
    }


def inspect(config):
    need(config["schema"] == "elastos.update-hop.fixture/v1", "unknown fixture schema")
    need(config["approval"].strip(), "exact fixture approval reference required")
    need(config["proof_kind"] in ("real-runtime", "harness-self-test"), "unknown proof kind")
    need(sys.platform == "darwin" or config["proof_kind"] == "harness-self-test", "real UP-00 proof targets native Mac")
    root = Path(config["root"])
    need(root.is_absolute() and root.is_dir(), "prepared stable fixture root required")
    need(not any(part in ("tmp", "private", "target") for part in root.parts), "use a stable task-owned fixture root")
    need(root.resolve() == root, "fixture root must use its physical path")
    need(config["old"]["version"] != config["new"]["version"], "same-version input cannot test an update hop")
    need(config["old"]["binary_sha256"] != config["new"]["binary_sha256"], "update requires different binaries")
    for binding in (config["source"], config["old"]["source"], config["new"]["source"]):
        for stamp in (binding["commit"], binding["tree"]):
            need(re.fullmatch(r"[0-9a-f]{40}", stamp), "exact source commit/tree required")
    ports = list(config["ports"].values())
    need(len(set(ports)) == 3 and all(isinstance(p, int) and 1024 <= p <= 65535 for p in ports), "three distinct unprivileged ports required")
    for role in ("publisher", "controller", "cli", "operator"):
        need(digest(binary(config, role)) == config["old"]["binary_sha256"], role + " old binary differs")
        need((data(config, role) / "identity/device.key").stat().st_size == 32, role + " prepared fixture identity required")
        if (data(config, role) / "components.json").exists():
            fixture_components(data(config, role) / "components.json")
        need(lock_state(data(config, role) / "host-process.lock") != "held", role + " already has a host")
        # Prepared operator fixtures keep providers and remote peers out of scope.
        for name in ("collaboration-network-v1.json", "operator-control.json"):
            path = data(config, role) / name
            need(not path.exists(), role + " fixture must start without " + name)
        need(not files(data(config, role) / "bin"), role + " fixture has installed provider binaries")
        need(not any(p.is_symlink() for p in home(config, role).rglob("*")), role + " fixture contains a symlink")
        need(all(p.stat().st_nlink == 1 for p in home(config, role).rglob("*") if p.is_file()), role + " fixture contains a shared hard link")
    for role in ("cli", "operator"):
        need(digest(data(config, role) / "components.json") == config["old"]["components_sha256"], role + " components differ")
        stored = source(config, role)
        need(len(read(data(config, role) / "sources.json")["sources"]) == 1, "fixture must have one local source")
        need(stored["installed_version"] == config["old"]["version"], role + " source is not the older state")
        need(stored["install_path"] == str(binary(config, role)), role + " install path escapes its fixture")
        need(stored["channel"] == config["channel"] and config["channel"] != "stable", "use one explicit test channel")
        need(not stored.get("gateways"), "fixture source must use only its local publisher")
        need(set(config["preserve"][role]) == {"identity", "chat", "draft", "library", "model_stub"}, "all preservation fixtures required")
        need(config["preserve"][role]["identity"] == "identity/device.key", "preserve the canonical fixture identity")
        for relative in config["preserve"][role].values():
            need(bool(files(inside(data(config, role), relative))), "preservation fixture is empty or absent")
        need(bool(files(inside(data(config, role), config["catalogue"]))), "catalogue fixture required")
    publisher = data(config, "publisher") / "ElastOS/SystemServices/Publisher"
    arch = "aarch64" if platform.machine() == "arm64" else "x86_64"
    release_platform = arch + "-darwin"
    paths = {"head": publisher / "release-head.json", "release": publisher / "release.json",
             "binary": publisher / "artifacts" / ("elastos-" + release_platform),
             "components": publisher / "artifacts" / ("components-" + release_platform + ".json")}
    for key, path in paths.items():
        inside(root, str(path.relative_to(root)))
        expected = config["publication"][key] if key in ("head", "release") else config["new"][key + "_sha256"]
        need(digest(path) == expected, "publication " + key + " differs")
    fixture_components(paths["components"])
    growth = 2 * sum(paths[key].stat().st_size for key in ("binary", "components"))
    need(shutil.disk_usage(root).free >= growth, "two update copies would not fit on the fixture volume")
    head, release = read(paths["head"]), read(paths["release"])
    for envelope in (head, release):
        need(envelope["payload"]["version"] == config["new"]["version"], "publication version differs")
        need(envelope["payload"]["channel"] == config["channel"], "publication channel differs")
    need(head["payload"]["release_sha256"] == digest(paths["release"]), "head envelope binding differs")
    for key in ("binary", "components"):
        need(release["payload"]["platforms"][release_platform][key]["sha256"] == digest(paths[key]), "release artifact binding differs")
    for role in ("cli", "operator"):
        need(source(config, role)["publisher_dids"] == [head["signer_did"]], "fixture signer differs")
    artifacts = {key: {"sha256": digest(path), "bytes": path.stat().st_size} for key, path in paths.items()}
    artifacts["components"]["semantic_sha256"] = json_digest(paths["components"])
    return artifacts


def snapshot(config, role, process, version):
    directory = data(config, role)
    try:
        installed_version = source(config, role)["installed_version"]
    except (OSError, ValueError, KeyError, StopIteration):
        installed_version = None
    return {"binary_sha256": digest(binary(config, role)) if binary(config, role).is_file() else None,
            "components_sha256": digest(directory / "components.json") if (directory / "components.json").is_file() else None,
            "components_semantic_sha256": json_digest(directory / "components.json") if (directory / "components.json").is_file() else None,
            "binary_version": version,
            "catalogue": files(inside(directory, config["catalogue"])),
            "preserved": {key: files(inside(directory, relative)) for key, relative in config["preserve"][role].items()},
            "installed_version": installed_version,
            "host_exit": process.poll(), "healthy": health(config["ports"][role]),
            "host_lock": lock_state(directory / "host-process.lock")}


def compare(observation):
    checks = {}
    def check(name, passed, reason):
        checks[name] = {"status": "passed" if passed else "failed", "reason": reason}
    before, after, new = (observation[k] for k in ("before", "after", "new"))
    check("actual_version_change_attempt", before["installed_version"] != new["version"] and observation["attempted"], "apply must target a different declared version")
    check("apply_command", observation["apply_exit"] == 0, "exit=" + str(observation["apply_exit"]))
    for key in ("binary_sha256", "components_sha256"):
        check(key, after[key] == new[key], "installed hash compared with the supplied new artifact")
    if after.get("components_semantic_sha256") and observation.get("new_components_semantic_sha256"):
        check("components_content", after["components_semantic_sha256"] == observation["new_components_semantic_sha256"], "installed component JSON compared with the verified new artifact")
    else:
        checks["components_content"] = {"status": "unavailable", "reason": "older observation lacks the component content digest; derive it separately from retained files"}
    check("source_version", after["installed_version"] == new["version"], "sources.json must record the new version")
    check("binary_version", after["binary_version"] == "elastos " + new["version"], "exact --version output compared with the supplied version")
    check("catalogue", after["catalogue"] == {".": new["catalogue_sha256"]}, "installed catalogue compared with the supplied new catalogue hash")
    for key, value in before["preserved"].items():
        check("preserve_" + key, bool(value) and value == after["preserved"][key], "before/after fixture file hashes; model stub is preservation only")
    check("host_survival", after["host_exit"] is None, "host exit=" + str(after["host_exit"]))
    check("health", after["healthy"], "HTTP /healthz after the observation window")
    check("host_lock", after["host_lock"] == ("held" if after["host_exit"] is None else "released"), "observed lock=" + after["host_lock"])
    if observation["role"] == "operator":
        reproduced = (checks["actual_version_change_attempt"]["status"] == "passed"
                      and after["host_exit"] == 75 and after["binary_sha256"] == new["binary_sha256"]
                      and checks["components_content"]["status"] == "passed"
                      and after["binary_version"] == "elastos " + new["version"]
                      and after["installed_version"] == before["installed_version"]
                      and observation.get("cache_stage_observed") and observation.get("operator_response_lost")
                      and observation["apply_exit"] != 0)
        if reproduced:
            checks["G6"] = {"status": "failed", "reason": "host exited 75 after verified replacement and the cache stage, before sources.json advanced; the operator lost its response; the wait boundary has no direct log marker"}
        elif checks["apply_command"]["status"] == "passed" and checks["source_version"]["status"] == "passed":
            checks["G6"] = {"status": "passed", "reason": "operator apply completed and saved the new source version in this run"}
        else:
            checks["G6"] = {"status": "unavailable", "reason": "this failure does not establish or reject the specific self-update exit hypothesis"}
    else:
        checks["G6"] = {"status": "unavailable", "reason": "G6 concerns the operator apply path"}
    for name in ("stage", "restart", "undo"):
        checks[name] = {"status": "unavailable", "reason": "unsupported by the current updater; harness performs no substitute operation"}
    checks["external_assets"] = {"status": "unavailable", "reason": "bounded fixture has an empty external component map"}
    return checks


def unavailable_path(reason):
    names = ("actual_version_change_attempt", "apply_command", "binary_sha256", "components_sha256",
             "components_content", "source_version", "binary_version", "catalogue", "preserve_identity", "preserve_chat",
             "preserve_draft", "preserve_library", "preserve_model_stub", "host_survival", "health",
             "host_lock", "G6", "stage", "restart", "undo", "external_assets")
    return {"status": "unavailable", "reason": reason,
            "checks": {name: {"status": "unavailable", "reason": reason} for name in names}}


def run(config, output):
    artifacts = inspect(config)
    output.mkdir(mode=0o700, parents=True, exist_ok=False)
    processes, logs = [], []
    result = {"schema": "elastos.update-hop.result/v1", "proof_kind": config["proof_kind"],
              "source": config["source"], "old": config["old"], "new": config["new"],
              "approval": config["approval"], "artifacts": artifacts,
              "started_at": datetime.datetime.now(datetime.timezone.utc).isoformat(), "paths": {}}

    def spawn(role, args, label):
        log = (output / (label + ".log")).open("wb")
        logs.append(log)
        proc = subprocess.Popen([str(binary(config, role)), *args], env=environment(config, role),
                                cwd=home(config, role), stdout=log, stderr=subprocess.STDOUT,
                                stdin=subprocess.DEVNULL, start_new_session=True)
        processes.append(proc)
        return proc

    def command(role, args, label, timeout=30):
        proc = spawn(role, args, label)
        try:
            return proc.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            os.killpg(proc.pid, signal.SIGKILL)
            proc.wait()
            return 124

    def local_info(role):
        label = role + "-info"
        need(command(role, ["node", "info", "--json"], label) == 0, label + " failed")
        return read(output / (label + ".log"))

    def info(role):
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        with opener.open(f"http://127.0.0.1:{config['ports'][role]}/.well-known/elastos/carrier-bootstrap.json?role=publisher", timeout=3) as response:
            bootstrap = json.load(response)
        need(bootstrap.get("schema") == "elastos.carrier.bootstrap/v1", role + " Carrier bootstrap unavailable")
        return {"did": bootstrap["did"], "connect_ticket": bootstrap["ticket"]}

    def observe(role, host, label):
        version = "unavailable"
        try:
            if command(role, ["--version"], label, timeout=5) == 0:
                version = (output / (label + ".log")).read_text().strip()
        except OSError as error:
            version = "execution failed: " + str(error)
        return snapshot(config, role, host, version)

    def start(role):
        port = config["ports"][role]
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", port))
        proc = spawn(role, ["gateway", "--addr", f"127.0.0.1:{port}", "--cache-dir", str(home(config, role) / "gateway-cache")], role + "-host")
        deadline = time.monotonic() + 30
        while proc.poll() is None and time.monotonic() < deadline:
            if health(port) and lock_state(data(config, role) / "host-process.lock") == "held":
                return proc
            time.sleep(0.2)
        raise ValueError(role + " host failed readiness")

    try:
        start("publisher")
        publisher = info("publisher")
        need(publisher.get("connect_ticket"), "local publisher ticket unavailable")
        for role in ("cli", "operator"):
            stage = "fixture preparation"
            entry = result["paths"][role] = unavailable_path("path has not reached apply")
            entry["stage"] = stage
            try:
                path = data(config, role) / "sources.json"
                stored = read(path)
                selected = next(s for s in stored["sources"] if s["name"] == stored["default_source"])
                need(selected["publisher_dids"] == [publisher["did"]], "local publisher identity differs from signed fixture")
                selected.update(connect_ticket=publisher["connect_ticket"], publisher_node_id="", discovery_uri="", ipns_name="")
                write(path, stored)
                if role == "operator":
                    # Read fixture identity before its gateway can hold the lock.
                    target_did = local_info(role)["did"]
                    need(isinstance(target_did, str) and target_did, "operator fixture identity unavailable")
                host = start(role)
                if role == "operator":
                    controller, target = local_info("controller"), info(role)
                    need(target["did"] == target_did, "operator target identity differs from its gateway bootstrap")
                    need(target.get("connect_ticket"), "operator target ticket unavailable")
                    need(command(role, ["node", "peer", "add", "--did", controller["did"], "--allow", "status.read", "--allow", "update.check", "--allow", "update.apply"], "target-peer") == 0, "target peer admission failed")
                    need(command("controller", ["node", "peer", "add", "--did", target["did"], "--ticket", target["connect_ticket"]], "controller-peer") == 0, "controller peer admission failed")
                    actor, args = "controller", ["node", "update", "--peer", target["did"], "--apply", "--yes", "--json"]
                else:
                    actor, args = role, ["update", "--yes"]
                before = observe(role, host, role + "-version-before")
                need(before["binary_version"] == "elastos " + config["old"]["version"], "old binary version differs")
                stage = "apply"
                exit_code = command(actor, args, role + "-apply", timeout=60)
                # Current host watch polls each second; migration waits at most 15 s.
                time.sleep(2)
                after = observe(role, host, role + "-version-after")
                host_text = (output / (role + "-host.log")).read_text(errors="replace")
                command_text = (output / (role + "-apply.log")).read_text(errors="replace")
                apply_text = host_text + "\n" + command_text
                observation = {"role": role, "new": config["new"], "before": before, "after": after,
                               "new_components_semantic_sha256": artifacts["components"]["semantic_sha256"],
                               "apply_exit": exit_code,
                               **update_evidence(config, before, after, host_text, command_text)}
                write(output / (role + "-observation.json"), observation)
                checks = compare(observation)
                stages = ["Installing ", "Downloading binary", "Binary verified", "Downloading components",
                          "Components verified", "Installed binary:", "Installed binary verified",
                          "Installed components:", "support assets", "Capsule cache", "Principal-root readiness:",
                          "installed successfully!"]
                observed = [s for s in stages if s in apply_text]
                entry.pop("reason", None)
                entry.update(status="failed" if any(c["status"] == "failed" for c in checks.values()) else "passed", checks=checks,
                             stage="observation", apply_exit=exit_code,
                             last_update_stage=observed[-1] if observed else "see private apply log",
                             apply_log_sha256=digest(output / (role + "-apply.log")))
            except (OSError, ValueError, KeyError, StopIteration) as error:
                entry.update(unavailable_path(str(error)), stage=stage)
    except (OSError, ValueError, KeyError, StopIteration, KeyboardInterrupt) as error:
        result["prerequisite_failure"] = str(error)
    finally:
        cleanup, cleanup_errors = [], []
        for proc in reversed(processes):
            proc.poll()  # Reap dead group leaders before signalling on macOS.
            try:
                os.killpg(proc.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            except OSError as error:
                cleanup_errors.append(str(error))
        time.sleep(0.2)
        for proc in reversed(processes):
            proc.poll()
            try:
                os.killpg(proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            except OSError as error:
                cleanup_errors.append(str(error))
            try:
                proc.wait(timeout=3)
            except subprocess.TimeoutExpired:
                cleanup_errors.append("process survived cleanup: " + str(proc.pid))
            cleanup.append({"pid": proc.pid, "exit": proc.returncode})
        for log in logs:
            log.close()
        result["cleanup"] = {"owned_processes": cleanup, "errors": cleanup_errors, "fixtures": "retained for inspection; no fixture deletion or reset",
                             "locks": {r: lock_state(data(config, r) / "host-process.lock") for r in ("publisher", "cli", "operator")},
                             "health": {r: health(config["ports"][r]) for r in ("publisher", "cli", "operator")}}
        if "held" in result["cleanup"]["locks"].values() or any(result["cleanup"]["health"].values()):
            cleanup_errors.append("fixture still has an active host lock or health endpoint")
        for role in ("cli", "operator"):
            result["paths"].setdefault(role, unavailable_path("prerequisite failed before this path"))
        write(output / "result.json", result)
    print(str(output / "result.json"))
    return 0 if not cleanup_errors and all(p["status"] == "passed" for p in result["paths"].values()) else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("operation", choices=("inspect", "run", "compare"))
    parser.add_argument("input", type=Path)
    parser.add_argument("output", type=Path, nargs="?")
    args = parser.parse_args()
    os.umask(0o077)
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt("terminated")))
    try:
        value = read(args.input)
        if args.operation == "run":
            need(args.output is not None and args.output.is_absolute(), "run requires a new absolute receipt directory")
            return run(value, args.output)
        print(json.dumps(inspect(value) if args.operation == "inspect" else compare(value), indent=2))
        return 0
    except (OSError, ValueError, KeyError, StopIteration) as error:
        print(json.dumps({"status": "unavailable", "reason": str(error)}), file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
