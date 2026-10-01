#!/usr/bin/env python3
"""Run journeys against this job's installed Home; save only public proof."""
import argparse
import hashlib
import json
import os
import signal
import socket
import subprocess
import time
import urllib.request
from pathlib import Path


def digest(path):
    value = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


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
              "fixture_components_sha256": digest(data / "components.json"), "results": {}}
    environment = dict(os.environ, HOME=str(home), XDG_DATA_HOME=str(home / ".local/share"))
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
        subprocess.run(["node", "scripts/ci-installed-home-journey.mjs", f"http://{address}", str(evidence)], env=environment, check=True)
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
        record["elapsed_seconds"] = round(time.monotonic() - started, 2)
        if (evidence / "home-journey.json").exists():
            record["results"].update(json.loads((evidence / "home-journey.json").read_text())["results"])
        (evidence / "installed-journeys.json").write_text(json.dumps(record, indent=2) + "\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("home", type=Path)
    parser.add_argument("data", type=Path)
    parser.add_argument("evidence", type=Path)
    args = parser.parse_args()
    run(args.home.resolve(), args.data.resolve(), args.evidence.resolve())
