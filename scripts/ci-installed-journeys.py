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
import time
import urllib.request
from pathlib import Path

TIMING = runpy.run_path(str(Path(__file__).with_name("ci-installed-model-timing.py")))


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


def run(home, data, evidence):
    installed = data / "bin/elastos"
    receipt = json.loads((data / "receipts/source-home-installation.json").read_text())
    runtime_sha = digest(installed)
    candidate = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    # Inspect the installation receipt and the installed bytes before launch.
    assert candidate == receipt["source"]["commit"]
    assert runtime_sha == receipt["runtime"]["installed_sha256"].removeprefix("sha256:")
    record = {"candidate": candidate, "source_tree": subprocess.check_output(["git", "rev-parse", "HEAD^{tree}"], text=True).strip(),
              "installed_runtime_sha256": runtime_sha, "installation_receipt_sha256": digest(data / "receipts/source-home-installation.json"),
              "installed_model_provider_sha256": digest(data / "bin/model-provider"),
              "fixture_components_sha256": digest(data / "components.json"), "results": {}}
    record["disk_before"] = disk_observation(data)
    (evidence / "installed-journeys.json").write_text(json.dumps(record, indent=2) + "\n")
    if record["disk_before"]["available_bytes"] * 100 < record["disk_before"]["capacity_bytes"] * 12:
        raise RuntimeError("installed journey requires 12% free disk")
    environment = dict(os.environ, HOME=str(home), XDG_DATA_HOME=str(data.parent),
                       ELASTOS_MODEL_TIMING_DIAGNOSTICS="1")
    with socket.socket() as port:
        port.bind(("127.0.0.1", 0))
        number = port.getsockname()[1]
        address = f"localhost:{number}"
        bind_address = f"127.0.0.1:{number}"
    started = time.monotonic()
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
        observer = TIMING["ModelTimingObserver"](child, data, started)
        try:
            with observer:
                subprocess.run(["node", "scripts/ci-installed-home-journey.mjs", f"http://{address}", str(evidence), str(data)], env=environment, check=True)
        finally:
            record["model_timing"] = observer.receipt()
        record["results"].update(json.loads((evidence / "home-journey.json").read_text())["results"])
    finally:
        try:
            os.killpg(child.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            child.wait(timeout=15)
        except subprocess.TimeoutExpired:
            os.killpg(child.pid, signal.SIGKILL)
            child.wait(timeout=5)
        log.close()
        timing = record.setdefault("model_timing", {})
        timing["provider_stages"] = TIMING["stage_timings"](home / "journey-runtime.private.log")
        timing["acknowledgements"] = TIMING["acknowledgement_timings"](home / "journey-runtime.private.log")
        timing["durations"] = TIMING["run_metrics"](timing["provider_stages"], timing["acknowledgements"])
        record["disk_after"] = disk_observation(data)
        record["elapsed_seconds"] = round(time.monotonic() - started, 2)
        if (evidence / "home-journey.json").exists():
            record["results"].update(json.loads((evidence / "home-journey.json").read_text())["results"])
        (evidence / "installed-journeys.json").write_text(json.dumps(record, indent=2) + "\n")

    if record["model_timing"]["durations"]["status"] != "complete":
        raise RuntimeError("installed reply timing evidence is incomplete")


def fresh_fixture(home, data, destination):
    """Copy installed code and receipts; each run owns all mutable Home state."""
    usage = disk_observation(data)
    if usage["available_bytes"] * 100 < usage["capacity_bytes"] * 15:
        raise RuntimeError("fresh journey fixture requires 15% free disk")
    destination.mkdir(parents=True, exist_ok=False)
    target = destination / data.relative_to(home)
    target.mkdir(parents=True)
    for name in ("bin", "capsules", "libexec", "receipts", "scripts"):
        if (data / name).exists():
            shutil.copytree(data / name, target / name, symlinks=True)
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
            subprocess.run(["node", "scripts/ci-model-package.mjs", str(run_data),
                            str(home / "model-inputs/inputs"), str(run_evidence)], check=True)
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
    args = parser.parse_args()
    operation = repeat if args.repeat else run
    arguments = [args.home.resolve(), args.data.resolve(), args.evidence.resolve()]
    if args.repeat:
        arguments.append(args.repeat)
    operation(*arguments)
