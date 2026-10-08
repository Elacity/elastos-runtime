#!/usr/bin/env python3
"""Run journeys against this job's installed Home; save only public proof."""
import argparse
import hashlib
import json
import os
import runpy
import signal
import shutil
import socket
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

TIMING = runpy.run_path(str(Path(__file__).with_name("ci-installed-model-timing.py")))
UPSTREAM = runpy.run_path(str(Path(__file__).with_name("release-upstream-input.py")))
DISK_RESERVE_BYTES = 2 * 1024 ** 3


def digest(path):
    value = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def disk_observation(data):
    usage = os.statvfs(data)
    return {"capacity_bytes": usage.f_blocks * usage.f_frsize,
            "available_bytes": usage.f_bavail * usage.f_frsize}


def require_disk_space(data, needed=0):
    usage = disk_observation(data)
    if usage["available_bytes"] - needed < DISK_RESERVE_BYTES:
        raise RuntimeError("installed journey requires a 2 GiB free disk reserve")
    return usage


def engine_paths(data):
    installation = json.loads((data / "receipts/source-home-installation.json").read_text())
    manifest = json.loads((data / "components.json").read_text())
    platform = installation["platform"]
    component = manifest["external"]["llama-server"]
    info = component["platforms"][platform]
    bundle = data / UPSTREAM["relative"](info["install_path"])
    return platform, component, info, bundle


def engine_receipt(data):
    platform, component, info, bundle = engine_paths(data)
    path = bundle / ".elastos-engine.json"
    if not path.is_file():
        raise RuntimeError("source Home requires SETUP_SOURCE_HOME_INSTALL_LLAMA_SERVER=1 before the model journey")
    receipt = json.loads(UPSTREAM["regular"](path).read_bytes())
    assert receipt["schema"] == "elastos.local-model-engine/v2"
    assert receipt["platform"] == platform and receipt["version"] == component["version"]
    assert receipt["archive_sha256"] == info["checksum"]
    entries = receipt["entries"]
    assert isinstance(entries, list) and 0 < len(entries) <= 1024
    names = set()
    for entry in entries:
        name = entry["path"] if entry["path"] == "_elastos_object.json" else UPSTREAM["relative"](entry["path"])
        assert entry["type"] == "file" and name not in names
        names.add(name)
        assert "sha256:" + digest(UPSTREAM["regular"](bundle / name)) == entry["sha256"]
    binary = UPSTREAM["relative"](info["binary_path"])
    assert binary in names and os.access(bundle / binary, os.X_OK)
    recipes = json.loads(Path("scripts/release-upstream-recipes.json").read_bytes())["recipes"]
    recipe, = [row for row in recipes if row["component"] == "llama-server" and row["platform"] == platform]
    assert recipe["install_path"] == info["install_path"] and recipe["binary_path"] == binary
    assert recipe["root"] == "llama-" + component["version"]
    provenance = json.loads(UPSTREAM["regular"](bundle / "PROVENANCE.json").read_bytes())
    recipe_sha = hashlib.sha256(UPSTREAM["canonical"](UPSTREAM["public_recipe"](recipe))).hexdigest()
    assert provenance["recipe_sha256"] == recipe_sha
    assert provenance["upstream"] == UPSTREAM["source_record"](recipe["source"])
    return {"scope": "preinstalled source prerequisite; Runtime Use verifies this engine",
            "signed_release_on_demand_acquisition": "separate acceptance proof",
            "platform": platform, "version": component["version"],
            "archive_sha256": receipt["archive_sha256"], "recipe_sha256": recipe_sha,
            "receipt_sha256": digest(path), "executable_sha256": digest(bundle / binary)}


def process_rows():
    output = subprocess.check_output(["ps", "-axo", "pid=,ppid=,lstart=,args="], text=True, timeout=5)
    rows = {}
    for line in output.splitlines():
        fields = line.strip().split(None, 7)
        if len(fields) == 8:
            rows[int(fields[0])] = {"ppid": int(fields[1]), "start": " ".join(fields[2:7]),
                                   "command": fields[7]}
    return rows


def owned_processes(rows, runtime_pid, data):
    owned = {}
    root_owned = rows.get(runtime_pid, {}).get("command", "").startswith(str(data / "bin/elastos"))
    for pid, row in rows.items():
        command = row["command"]
        path_owned = command.startswith(str(data / "bin") + "/") or command.startswith(str(data / "libexec") + "/")
        parent, visited = pid, set()
        while parent in rows and parent not in visited and parent != runtime_pid:
            visited.add(parent)
            parent = rows[parent]["ppid"]
        if not path_owned and (not root_owned or parent != runtime_pid):
            continue
        role = ("runtime" if pid == runtime_pid else
                "guard" if " --internal-local-llama-guard" in command else
                "model_provider" if command.startswith(str(data / "bin/model-provider")) else
                "llama_server" if "/llama-server" in command else "other")
        owned[pid] = dict(row, role=role)
    return owned


def process_counts(rows):
    return {role: sum(row["role"] == role for row in rows.values())
            for role in ("runtime", "model_provider", "guard", "llama_server", "other")}


def cleanup_runtime(child, data, allow_group=True):
    try:
        known = owned_processes(process_rows(), child.pid, data)
    except (OSError, ValueError, subprocess.SubprocessError):
        known = None
    before = process_counts(known or {})
    def signal_runtime_group(action):
        # poll() can reap an exited Runtime. Its former group id then needs the
        # same birth/command proof as every individual descendant before a signal.
        original = (known or {}).get(child.pid)
        if original is None or original["role"] != "runtime" or child.poll() is not None:
            return
        latest = process_rows().get(child.pid)
        if latest and latest["start"] == original["start"] and latest["command"] == original["command"]:
            try:
                if allow_group:
                    os.killpg(child.pid, action)
                else:
                    os.kill(child.pid, action)
            except ProcessLookupError:
                pass
    signal_runtime_group(signal.SIGTERM)
    try:
        child.wait(timeout=15)
    except subprocess.TimeoutExpired:
        signal_runtime_group(signal.SIGKILL)
        child.wait(timeout=5)
    if known is None:
        return {"status": "failed", "reason": "process census unavailable", "before": before}

    def survivors():
        rows = process_rows()
        current = owned_processes(rows, child.pid, data)
        # Keep observed children in scope after reparenting, with PID reuse checked.
        for pid, row in known.items():
            if pid in rows and rows[pid]["start"] == row["start"] and rows[pid]["command"] == row["command"]:
                current[pid] = dict(rows[pid], role=row["role"])
        known.update(current)
        return current

    remaining = survivors()
    for action in (signal.SIGTERM, signal.SIGKILL):
        if not remaining:
            break
        for pid, row in remaining.items():
            latest = process_rows().get(pid)
            if latest and latest["start"] == row["start"] and latest["command"] == row["command"]:
                try:
                    os.kill(pid, action)
                except ProcessLookupError:
                    pass
        deadline = time.monotonic() + 5
        while remaining and time.monotonic() < deadline:
            time.sleep(.1)
            remaining = survivors()
    return {"status": "passed" if not remaining else "failed",
            "before": before, "after": process_counts(remaining)}


def run(home, data, evidence, model=True):
    if (evidence / "home-journey.json").exists() or (evidence / "home-journey.json").is_symlink():
        raise RuntimeError("installed journey requires fresh UI evidence")
    preparation_started = time.monotonic()
    evidence.mkdir(parents=True, exist_ok=True)
    installed = data / "bin/elastos"
    receipt = json.loads((data / "receipts/source-home-installation.json").read_text())
    runtime_sha = digest(installed)
    candidate = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    # Inspect the installation receipt and the installed bytes before launch.
    source_tree = subprocess.check_output(["git", "rev-parse", "HEAD^{tree}"], text=True).strip()
    assert candidate == receipt["source"]["commit"] and source_tree == receipt["source"]["tree"]
    assert receipt["source"]["clean"] is True and receipt["runtime"]["parity"] is True
    assert runtime_sha == receipt["runtime"]["installed_sha256"].removeprefix("sha256:")
    assert receipt["runtime"]["built_sha256"] == receipt["runtime"]["installed_sha256"]
    assert "sha256:" + digest(data / "components.json") == receipt["components_sha256"]
    manifest = json.loads((data / "components.json").read_text())
    provider_sha = digest(data / "bin/model-provider")
    assert "sha256:" + provider_sha == manifest["external"]["model-provider"]["platforms"][receipt["platform"]]["checksum"]
    record = {"candidate": candidate, "source_tree": source_tree,
              "installed_runtime_sha256": runtime_sha, "installation_receipt_sha256": digest(data / "receipts/source-home-installation.json"),
              "installed_model_provider_sha256": provider_sha,
              "source_components_sha256": digest(data / "components.json"), "results": {}}
    if model:
        record["installed_engine"] = engine_receipt(data)
    else:
        _, _, _, bundle = engine_paths(data)
        absent_paths = (data / "bin/llama-server", bundle, data / "capsules/llama-server")
        assert all(not path.exists() and not path.is_symlink() for path in absent_paths)
        record["engine_absent"] = True
        record["engine_absent_after_ui"] = False
    record["disk_before"] = disk_observation(data)
    (evidence / "installed-journeys.json").write_text(json.dumps(record, indent=2) + "\n")
    require_disk_space(data)
    environment = dict(os.environ, HOME=str(home), XDG_DATA_HOME=str(data.parent),
                       ELASTOS_MODEL_TIMING_DIAGNOSTICS="1")
    inputs = Path(os.environ.get("CI_MODEL_INPUTS", str(home / "model-inputs/inputs")))
    # Package, local Kubo blocks and the admitted model cache can each own a copy.
    needed = 3 * sum(path.stat().st_size for path in inputs.iterdir() if path.is_file())
    require_disk_space(data, needed)
    subprocess.run(["node", "scripts/ci-model-package.mjs", str(data), str(inputs), str(evidence)],
                   env=environment, check=True)
    record["fixture_package_receipt_sha256"] = digest(evidence / "package.json")
    record["fixture_components_sha256"] = digest(data / "components.json")
    require_disk_space(data)
    prior = owned_processes(process_rows(), -1, data)
    record["processes_before_launch"] = process_counts(prior)
    if prior:
        raise RuntimeError("installed journey fixture already owns running processes")
    with socket.socket() as port:
        port.bind(("127.0.0.1", 0))
        number = port.getsockname()[1]
        address = f"localhost:{number}"
        bind_address = f"127.0.0.1:{number}"
    started = time.monotonic()
    record["fixture_preparation_seconds"] = round(started - preparation_started, 2)
    # Runtime logs stay private in the isolated Home; upload receipts/screenshots only.
    log = (home / "journey-runtime.private.log").open("wb")
    child = subprocess.Popen([str(installed), "gateway", "--addr", bind_address], env=environment, stdout=log, stderr=log, start_new_session=True)
    try:
        deadline = time.monotonic() + 120
        while time.monotonic() < deadline:
            assert child.poll() is None, "installed Runtime exited before health"
            try:
                with urllib.request.urlopen(f"http://{address}/healthz", timeout=1) as response:
                    if response.status == 200:
                        break
            except OSError:
                time.sleep(0.25)
        else:
            raise RuntimeError("installed Runtime health deadline")
        command = ["node", "scripts/ci-installed-home-journey.mjs", f"http://{address}", str(evidence), str(data)]
        if model:
            observer = TIMING["ModelTimingObserver"](child, data, started)
            try:
                with observer:
                    subprocess.run(command, env=environment, check=True)
            finally:
                record["model_timing"] = observer.receipt()
        else:
            subprocess.run([*command, "--home-only"], env=environment, check=True)
        ui = json.loads((evidence / "home-journey.json").read_text())
        record["results"].update(ui["results"])
        if not model:
            assert ui.get("dispatch_unavailable_reason") == "source_engine_required"
            record["dispatch_unavailable_reason"] = "source_engine_required"
            assert record["results"].get("engine_absent_refusal") == "passed"
            record["engine_absent_after_ui"] = all(not path.exists() and not path.is_symlink() for path in absent_paths)
            assert record["engine_absent_after_ui"]
            assert record["results"]["home_screenshots"] == "passed"
            record["results"]["engine_absent_home"] = "passed"
    finally:
        try:
            observer_complete = not model or record.get("model_timing", {}).get("observer_complete") is True
            record["process_cleanup"] = cleanup_runtime(child, data, allow_group=observer_complete)
        except (OSError, ValueError, subprocess.SubprocessError):
            record["process_cleanup"] = {"status": "failed", "reason": "process census unavailable"}
        finally:
            log.close()
            if model:
                timing = record.setdefault("model_timing", {})
                timing["provider_stages"] = TIMING["stage_timings"](home / "journey-runtime.private.log")
                timing["acknowledgements"] = TIMING["acknowledgement_timings"](home / "journey-runtime.private.log")
                timing["durations"] = (TIMING["run_metrics"](timing["provider_stages"], timing["acknowledgements"])
                                       if sys.platform == "darwin" else
                                       {"status": "unavailable_with_confined_provider_stderr"})
                record["results"]["model_timing_observer"] = "passed" if timing.get("observer_complete") is True else "failed"
        record["results"]["process_cleanup"] = record["process_cleanup"]["status"]
        record["disk_after"] = disk_observation(data)
        record["results"]["disk_reserve"] = ("passed" if record["disk_after"]["available_bytes"] >= DISK_RESERVE_BYTES else "failed")
        record["elapsed_seconds"] = round(time.monotonic() - started, 2)
        if (evidence / "home-journey.json").exists():
            ui = json.loads((evidence / "home-journey.json").read_text())
            record["results"].update(ui["results"])
        if not model:
            record["results"]["engine_absent_refusal"] = (
                "passed" if record.get("dispatch_unavailable_reason") == "source_engine_required"
                and record["engine_absent_after_ui"] and record["results"].get("engine_absent_refusal") == "passed"
                and record["process_cleanup"].get("before", {}).get("llama_server") == 0
                else "failed")
            record["results"]["engine_absent_home"] = "passed" if record["engine_absent_after_ui"] else "failed"
        (evidence / "installed-journeys.json").write_text(json.dumps(record, indent=2) + "\n")

    if record["process_cleanup"]["status"] != "passed":
        raise RuntimeError("installed journey left owned processes running")
    if not model and record["results"]["engine_absent_refusal"] != "passed":
        raise RuntimeError("installed engine-absence refusal proof is incomplete")
    if model and record["results"]["model_timing_observer"] != "passed":
        raise RuntimeError("installed timing observer did not stop")
    require_disk_space(data)
    if model and sys.platform == "darwin" and record["model_timing"]["durations"]["status"] != "complete":
        raise RuntimeError("installed reply timing evidence is incomplete")


def fresh_fixture(home, data, destination, include_engine=True):
    """Copy installed code and receipts; each run owns all mutable Home state."""
    _, _, _, engine_bundle = engine_paths(data)
    excluded = set() if include_engine else {data / "bin/llama-server", engine_bundle, data / "capsules/llama-server"}
    def ignore(directory, names):
        return [name for name in names if Path(directory) / name in excluded]
    immutable = ("bin", "capsules", "libexec", "receipts", "scripts")
    needed = sum(path.stat().st_size for name in immutable for path in (data / name).rglob("*")
                 if path.is_file() and not path.is_symlink() and not any(parent in excluded for parent in (path, *path.parents)))
    require_disk_space(data, needed)
    destination.mkdir(parents=True, exist_ok=False)
    target = destination / data.relative_to(home)
    target.mkdir(parents=True)
    for name in immutable:
        if (data / name).exists():
            shutil.copytree(data / name, target / name, symlinks=True, ignore=ignore)
    link = target / "bin/llama-server"
    if include_engine and link.is_symlink():
        _, _, info, _ = engine_paths(data)
        assert (data / "bin/llama-server").resolve() == engine_bundle / info["binary_path"]
        link.unlink()
        link.symlink_to(target / info["install_path"] / info["binary_path"])
    shutil.copy2(data / "components.json", target / "components.json")
    (target / "config.toml").write_text("dev_mode = true\ntrusted_keys = []\n")
    return target


def repeat(home, data, evidence, count):
    if count != 3:
        raise RuntimeError("Mac timing series requires three fresh journeys")
    failures = 0
    fixture_root = home.parent / "reply-repeat-homes"
    fixture_root.mkdir(exist_ok=False)
    for index in range(1, count + 1):
        run_home = fixture_root / f"run-{index}"
        run_evidence = evidence / f"run-{index}"
        run_evidence.mkdir(exist_ok=False)
        # Copy only installed immutable artifacts, never identity/trust, model
        # provider journal, engine processes, passkeys or IPFS mutable state.
        run_data = fresh_fixture(home, data, run_home)
        try:
            run(run_home, run_data, run_evidence)
        except (RuntimeError, AssertionError, subprocess.CalledProcessError):
            failures += 1
    if failures:
        raise RuntimeError(f"{failures} installed Mac timing journeys failed")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("home", type=Path)
    parser.add_argument("data", type=Path)
    parser.add_argument("evidence", type=Path)
    parser.add_argument("--repeat", type=int, choices=[3])
    parser.add_argument("--home-only", action="store_true")
    args = parser.parse_args()
    if args.home_only and args.repeat:
        parser.error("--home-only has one engine-absent Home")
    operation = repeat if args.repeat else run
    arguments = [args.home.resolve(), args.data.resolve(), args.evidence.resolve()]
    if args.repeat:
        arguments.append(args.repeat)
    elif args.home_only:
        home, data, evidence = arguments
        destination = home.parent / "engine-absent-home"
        arguments = [destination, fresh_fixture(home, data, destination, include_engine=False), evidence, False]
    else:
        home, data, evidence = arguments
        destination = home.parent / "reply-home"
        arguments = [destination, fresh_fixture(home, data, destination), evidence]
    operation(*arguments)
