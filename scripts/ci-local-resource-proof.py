#!/usr/bin/env python3
"""Prove installed CPU admission and cleanup in owned finite Linux units."""
import argparse
import hashlib
import json
import os
import platform
import re
import runpy
import shutil
import signal
import stat
import subprocess
import sys
import time
from pathlib import Path

HOME_HELPERS = runpy.run_path(str(Path(__file__).with_name("ci-installed-journeys.py")))
digest = HOME_HELPERS["digest"]
MODEL_SHA = "c4a3dd037301b6ecea31d6da37f5cd793ead920dd5ddfe6d589294628d6ce66a"
GIB = 1024 ** 3


def require(value, reason):
    if not value:
        raise RuntimeError(reason)


def load_json(path):
    def unique(pairs):
        result = {}
        for key, value in pairs:
            require(key not in result, "duplicate_json_key")
            result[key] = value
        return result
    return json.loads(path.read_text(), object_pairs_hook=unique)


def file_hash(path):
    info = path.lstat()
    require(stat.S_ISREG(info.st_mode) and info.st_uid == os.getuid()
            and not info.st_mode & 0o022, "unsafe_file")
    return "sha256:" + digest(path)


def relative_path(value):
    path = Path(value)
    require(value and not path.is_absolute() and all(p not in ("", ".", "..") for p in value.split("/")), "unsafe_relative_path")
    return path


def source_identity(root):
    def git(*args):
        return subprocess.check_output(["git", "-C", str(root), *args], text=True, timeout=15).strip()
    require(not git("status", "--porcelain", "--untracked-files=no"), "dirty_source")
    result = {"commit": git("rev-parse", "HEAD"), "tree": git("rev-parse", "HEAD^{tree}"), "clean": True}
    require(all(re.fullmatch("[0-9a-f]{40}", result[k]) for k in ("commit", "tree")), "source_identity")
    return result


def installed_identity(root, data, source, built_provider, built_runtime):
    receipt_path = data / "receipts/source-home-installation.json"
    receipt = load_json(receipt_path)
    require(receipt["schema"] == "elastos.source-home.installation-receipt/v1"
            and receipt["source"] == source and receipt["source"].get("clean") is True, "installation_source")
    runtime = file_hash(data / "bin/elastos")
    require(receipt["runtime"]["parity"] is True and runtime == file_hash(built_runtime)
            == receipt["runtime"]["built_sha256"] == receipt["runtime"]["installed_sha256"], "runtime_parity")
    manifest = load_json(data / "components.json")
    host = "linux-arm64" if platform.machine() in ("aarch64", "arm64") else "linux-amd64"
    require(receipt["platform"] == host, "installation_platform")
    provider = file_hash(data / "bin/model-provider")
    info = manifest["external"]["model-provider"]["platforms"][host]
    require(info["install_path"] == "bin/model-provider" and info["checksum"] == provider
            == file_hash(built_provider), "provider_parity")
    # The disposable signed catalogue changes components.json after installation.
    # Bind that fixture manifest separately from the source installation receipt.
    return manifest, host, {"candidate": source["commit"], "source_tree": source["tree"],
            "installation_receipt_sha256": file_hash(receipt_path),
            "fixture_components_sha256": file_hash(data / "components.json"),
            "runtime_sha256": runtime, "provider_sha256": provider}


def bundle_identity(bundle, component, host):
    metadata = bundle.lstat()
    require(stat.S_ISDIR(metadata.st_mode) and metadata.st_uid == os.getuid()
            and stat.S_IMODE(metadata.st_mode) == 0o500, "engine_root")
    receipt_path = bundle / ".elastos-engine.json"
    metadata = receipt_path.lstat()
    require(stat.S_ISREG(metadata.st_mode) and metadata.st_nlink == 1
            and stat.S_IMODE(metadata.st_mode) == 0o400, "engine_receipt_file")
    receipt = load_json(receipt_path)
    info = component["platforms"][host]
    require(set(receipt) == {"schema", "platform", "version", "archive_sha256", "entries"}
            and receipt["schema"] == "elastos.local-model-engine/v2" and receipt["platform"] == host
            and receipt["version"] == component["version"] and receipt["archive_sha256"] == info["checksum"], "engine_receipt")
    entries = []
    for path in sorted(bundle.rglob("*")):
        if path == receipt_path:
            continue
        metadata = path.lstat()
        require(metadata.st_uid == os.getuid(), "engine_owner")
        name = path.relative_to(bundle).as_posix()
        if stat.S_ISDIR(metadata.st_mode):
            require(stat.S_IMODE(metadata.st_mode) == 0o500, "engine_directory_mode")
        elif stat.S_ISREG(metadata.st_mode):
            require(metadata.st_nlink == 1 and stat.S_IMODE(metadata.st_mode) in (0o400, 0o500), "engine_file_mode")
            entries.append({"path": name, "type": "file", "sha256": file_hash(path)})
        elif stat.S_ISLNK(metadata.st_mode):
            target = os.readlink(path)
            relative_path(target)
            require(path.resolve().is_relative_to(bundle.resolve()), "engine_symlink")
            entries.append({"path": name, "type": "symlink", "target": target})
        else:
            raise RuntimeError("engine_special_file")
    require(entries == receipt["entries"] and stat.S_IMODE(receipt_path.lstat().st_mode) == 0o400, "engine_inventory")
    binary = bundle / relative_path(info["binary_path"])
    require(binary.name == "llama-server" and os.access(binary, os.X_OK), "engine_executable")
    return {"engine_sha256": file_hash(binary), "engine_receipt_sha256": file_hash(receipt_path),
            "engine_bundle_sha256": "sha256:" + hashlib.sha256(json.dumps(entries, sort_keys=True).encode()).hexdigest()}


def cgroup_observation(expected, membership=None, mounts=None, root=Path("/sys/fs/cgroup"), read=None):
    read = read or (lambda p: p.read_text())
    membership = read(Path("/proc/self/cgroup")) if membership is None else membership
    mounts = read(Path("/proc/self/mountinfo")) if mounts is None else mounts
    lines = [line.split(":", 2) for line in membership.splitlines()]
    require(all(len(line) == 3 for line in lines), "cgroup_membership")
    groups = [line[2] for line in lines if line[:2] == ["0", ""]]
    require(len(groups) == 1 and groups[0].startswith("/") and ".." not in groups[0].split("/")
            and not any("memory" in line[1].split(",") for line in lines), "cgroup_membership")
    entries = [line.split() for line in mounts.splitlines() if " - cgroup2 " in line]
    require(len(entries) == 1 and entries[0][3:5] == ["/", str(root)], "global_cgroup_mount")
    current = root / groups[0].lstrip("/")
    leaf = current
    ancestors = []
    while True:
        controllers = read(current / "cgroup.controllers").split()
        if current == root:
            try:
                read(current / "memory.max")
            except FileNotFoundError:
                require("memory" in controllers, "global_memory_controller")
                break
            raise RuntimeError("hidden_cgroup_root")
        maximum = read(current / "memory.max").strip()
        require(maximum == "max" or re.fullmatch("[0-9]+", maximum), "cgroup_numbers")
        limit = None if maximum == "max" else int(maximum)
        used_text = read(current / "memory.current").strip()
        require(re.fullmatch("[0-9]+", used_text), "cgroup_numbers")
        used = int(used_text)
        require((limit is None or limit > 0) and used >= 0, "cgroup_numbers")
        ancestors.append({"memory_max_bytes": limit, "memory_current_bytes": used})
        current = current.parent
    finite = [row["memory_max_bytes"] for row in ancestors if row["memory_max_bytes"] is not None]
    require(finite and ancestors[0]["memory_max_bytes"] == expected and min(finite) == expected, "cgroup_limit")
    require(read(leaf / "memory.swap.max").strip() == "0" and read(leaf / "pids.max").strip() == "512", "cgroup_swap_tasks")
    return {"global_root_visible": True, "effective_limit_bytes": expected, "swap_limit_bytes": 0,
            "tasks_limit": 512, "ancestors": ancestors}, leaf


def validate_outcome(row, qualification):
    require(qualification in ("low-memory", "lifecycle"), "fixture_qualification")
    common = {"qualification": qualification, "status": "passed", "final_descendants": 0, "account_lease_released": True}
    expected = dict(common, **({"error_class": "context_rejected", "error_code": "model_memory_unavailable",
             "actionable_message": True, "guard_start_observed": False} if qualification == "low-memory" else
             {"reply": True, "shared_busy": True, "idle_release": True, "crash_cleanup": True, "recovery": True}))
    require(row == expected and all(type(row[k]) is type(v) for k, v in expected.items()), "fixture_outcome")
    return row


def unit_processes(leaf):
    processes = set(map(int, (leaf / "cgroup.procs").read_text().split()))
    for child in leaf.iterdir():
        if child.is_dir():
            processes.update(unit_processes(child))
    return processes


def unit(fixture, test_binary, qualification, expected):
    plan = load_json(fixture / "plan.private.json")
    require(os.getuid() == plan["uid"] and os.getgid() == plan["gid"], "unit_account")
    observation, leaf = cgroup_observation(expected)
    require(file_hash(test_binary) == plan["test_executable_sha256"]
            and file_hash(fixture / "bin/model-provider") == plan["provider_sha256"], "unit_binary_identity")
    for name in ("first", "second"):
        require(file_hash(fixture / name / "model.gguf") == "sha256:" + MODEL_SHA, "unit_model_identity")
        require(bundle_identity(fixture / name / "engine", plan["engine_component"], plan["host"]) == plan["engine_identity"], "unit_engine_identity")
    environment = dict(os.environ, ELASTOS_MODEL_RESOURCE_PROOF_ROOT=str(fixture),
            ELASTOS_MODEL_RESOURCE_PROOF_LOW_MEMORY="1" if qualification == "low-memory" else "0",
            ELASTOS_MODEL_RESOURCE_PROOF_RESULT=str(fixture / "fixture-result.private.json"))
    with (fixture / "test.private.log").open("wb") as log:
        completed = subprocess.run([str(test_binary), "installed_local_resource_lifecycle", "--ignored", "--exact", "--nocapture"],
                env=environment, stdout=log, stderr=log, timeout=300)
    require(completed.returncode == 0, "fixture_test_failed")
    text = (fixture / "test.private.log").read_text()
    require(len(re.findall(r"^test installed_local_resource_lifecycle \.\.\. ok$", text, re.M)) == 1
            and "test result: ok. 1 passed; 0 failed; 0 ignored;" in text, "fixture_test_selection")
    outcome = validate_outcome(load_json(fixture / "fixture-result.private.json"), qualification)
    # Check before this main process exits; systemd's cleanup cannot mask orphans.
    remaining = unit_processes(leaf) - {os.getpid()}
    require(not remaining, "unit_descendants_remain")
    (fixture / "unit-result.private.json").write_text(json.dumps({"outcome": outcome, "cgroup": observation,
            "final_unit_descendants": 0, "checked_before_unit_exit": True}))


def unit_state(unit_name):
    fields = subprocess.check_output(["sudo", "systemctl", "show", unit_name,
            "--property=ActiveState,SubState,Result,ExecMainStatus,MainPID,ControlGroup"], text=True, timeout=15)
    return dict(line.split("=", 1) for line in fields.splitlines())


def wait_for_unit(unit_name, fixture, limit):
    deadline = time.monotonic() + 630
    while time.monotonic() < deadline:
        state = unit_state(unit_name)
        if state.get("ActiveState") == "active" and state.get("SubState") == "exited":
            require(state.get("Result") == "success" and state.get("ExecMainStatus") == "0"
                    and state.get("MainPID") == "0", "unit_exit")
            row = load_json(fixture / "unit-result.private.json")
            require(row["checked_before_unit_exit"] is True and type(row["final_unit_descendants"]) is int
                    and row["final_unit_descendants"] == 0, "unit_cleanup_proof")
            group = state.get("ControlGroup", "")
            require(group.startswith("/system.slice/elastos-la04-") and ".." not in group.split("/"), "unit_cgroup_identity")
            leaf = Path("/sys/fs/cgroup") / group.lstrip("/")
            require(not unit_processes(leaf), "unit_descendants_remain")
            require(row["cgroup"]["effective_limit_bytes"] == limit, "unit_limit_receipt")
            row["retained_unit_descendants"] = 0
            return row
        require(state.get("ActiveState") in ("activating", "active"), "unit_failed")
        time.sleep(0.25)
    raise RuntimeError("unit_deadline")


def run_checked(root, home, data, record):
    require(sys.platform.startswith("linux"), "linux_required")
    minimum = 12 if os.environ.get("CI") == "true" else 15
    disk = HOME_HELPERS["disk_observation"](home)
    require(disk["available_bytes"] * 100 >= disk["capacity_bytes"] * minimum, "disk_reserve")
    record["disk_before"] = disk
    source = source_identity(root)
    host = "linux-arm64" if platform.machine() in ("aarch64", "arm64") else "linux-amd64"
    target = Path(os.environ.get("CARGO_TARGET_DIR", "")) if os.environ.get("CARGO_TARGET_DIR") else None
    if target and not target.is_absolute():
        target = root / target
    built_provider = (target or root / "capsules/model-provider/target") / "release/model-provider"
    built_runtime = (target or root / "elastos/target") / "release/elastos"
    manifest, host, identities = installed_identity(root, data, source, built_provider, built_runtime)
    component = manifest["external"]["llama-server"]
    engine = data / relative_path(component["platforms"][host]["install_path"])
    engine_identity = bundle_identity(engine, component, host)
    record.update(identities, **engine_identity, model_sha256="sha256:" + MODEL_SHA)
    model = home / "model-inputs/inputs/SmolLM2-135M-Instruct-Q8_0.gguf"
    require(file_hash(model) == "sha256:" + MODEL_SHA, "pinned_model")
    private = home / ("resource-proof-" + os.environ.get("GITHUB_RUN_ID", "local") + "-" + os.environ.get("GITHUB_RUN_ATTEMPT", "1"))
    require(not private.exists(), "stale_fixture")
    needed = 4 * (sum(p.lstat().st_size for p in engine.rglob("*") if p.is_file() and not p.is_symlink()) + model.stat().st_size)
    require((disk["available_bytes"] - needed) * 100 >= disk["capacity_bytes"] * minimum, "copy_disk_reserve")
    private.mkdir(mode=0o700)
    # The existing verifier checks the installed component manifest before launch.
    record["stage"] = "installed-manifest"
    with (private / "manifest.private.log").open("wb") as log:
        subprocess.run(["bash", str(root / "scripts/installed-provider-verify.sh"), "--require-verified", "model-provider"],
            env=dict(os.environ, ELASTOS_DATA_DIR=str(data)), stdout=log, stderr=log, check=True, timeout=30)
    record["stage"] = "test-executable"
    compile_started = time.monotonic()
    with (private / "build.private.jsonl").open("wb") as log, (private / "build.private.log").open("wb") as errors:
        subprocess.run(["cargo", "test", "--locked", "--release", "--manifest-path", str(root / "capsules/model-provider/Cargo.toml"),
            "--test", "process", "--no-run", "--message-format=json"], cwd=root, stdout=log, stderr=errors, check=True, timeout=600)
    record["compile_elapsed_seconds"] = round(time.monotonic() - compile_started, 3)
    binaries = []
    for line in (private / "build.private.jsonl").read_text().splitlines():
        row = json.loads(line)
        if row.get("reason") == "compiler-artifact" and row.get("target", {}).get("name") == "process" and row.get("executable"):
            binaries.append(Path(row["executable"]))
    require(len(binaries) == 1, "test_executable_selection")
    test_binary = binaries[0]
    identities["test_executable_sha256"] = file_hash(test_binary)
    require(source_identity(root) == source and file_hash(built_provider) == identities["provider_sha256"], "post_build_identity")
    record.update(identities)
    results = record["qualifications"]
    for qualification, limit in (("low-memory", GIB), ("lifecycle", 4 * GIB)):
        record["stage"] = qualification
        qualification_started = time.monotonic()
        disk = HOME_HELPERS["disk_observation"](home)
        require((disk["available_bytes"] - needed) * 100 >= disk["capacity_bytes"] * minimum, "copy_disk_reserve")
        fixture = private / qualification
        fixture.mkdir(mode=0o700)
        (fixture / "bin").mkdir(mode=0o700)
        shutil.copyfile(data / "bin/model-provider", fixture / "bin/model-provider")
        (fixture / "bin/model-provider").chmod(0o700)
        for name in ("first", "second"):
            (fixture / name).mkdir(mode=0o700)
            shutil.copytree(engine, fixture / name / "engine", symlinks=True)
            shutil.copyfile(model, fixture / name / "model.gguf")
            (fixture / name / "model.gguf").chmod(0o400)
        plan = dict(identities, uid=os.getuid(), gid=os.getgid(), engine_component=component, host=host, engine_identity=engine_identity)
        (fixture / "plan.private.json").write_text(json.dumps(plan))
        unit_name = "elastos-la04-" + qualification + "-" + str(os.getpid())
        command = ["sudo", "systemd-run", "--quiet", "--unit=" + unit_name,
                "--service-type=exec",
                "--uid=" + str(os.getuid()), "--gid=" + str(os.getgid()), "-p", "MemoryMax=" + str(limit),
                "-p", "MemorySwapMax=0", "-p", "TasksMax=512", "-p", "RuntimeMaxSec=600",
                "-p", "KillMode=control-group", "-p", "RemainAfterExit=yes", "-p", "WorkingDirectory=" + str(root),
                "-p", "StandardOutput=append:" + str(fixture / "unit.private.log"),
                "-p", "StandardError=append:" + str(fixture / "unit.private.log"),
                "/usr/bin/env", "HOME=" + str(home), sys.executable, str(Path(__file__).resolve()),
                "unit", str(fixture), str(test_binary), qualification, str(limit)]
        try:
            with (fixture / "unit.private.log").open("wb") as log:
                subprocess.run(command, stdout=log, stderr=log, check=True, timeout=30)
            # Retain the unit until its result and empty final census are inspected.
            row = wait_for_unit(unit_name, fixture, limit)
            validate_outcome(row["outcome"], qualification)
            results[qualification] = row
        finally:
            with (fixture / "cleanup.private.log").open("wb") as log:
                subprocess.run(["sudo", "systemctl", "stop", unit_name], stdout=log, stderr=log, timeout=30, check=True)
                state = unit_state(unit_name)
                require(state.get("ActiveState") == "inactive" and state.get("MainPID") == "0"
                        and not state.get("ControlGroup"), "unit_stop")
                subprocess.run(["sudo", "systemctl", "reset-failed", unit_name], stdout=log, stderr=log, timeout=30)
            results[qualification]["explicit_stop_verified"] = True
        results[qualification]["elapsed_seconds"] = round(time.monotonic() - qualification_started, 3)
        record["results"]["la04_low_memory" if qualification == "low-memory" else "la04_lifecycle"] = "passed"
    record["qualification_elapsed_seconds"] = round(sum(row["elapsed_seconds"] for row in results.values()), 3)
    record.update(status="passed", stage="complete")


def run(root, home, data, evidence):
    record = {"status": "failed", "stage": "preflight", "qualifications": {}, "peak_memory_qualified": False,
              "results": {"la04_low_memory": "failed or not run", "la04_lifecycle": "failed or not run"}}
    started = time.monotonic()
    try:
        run_checked(root, home, data, record)
    finally:
        record["elapsed_seconds"] = round(time.monotonic() - started, 3)
        (evidence / "la04-result.json").write_text(json.dumps(record, indent=2) + "\n")


if __name__ == "__main__":
    def interrupted(signum, frame):
        raise RuntimeError("proof_interrupted")
    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)
    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=("run", "unit"))
    parser.add_argument("arguments", nargs="+")
    args = parser.parse_args()
    try:
        if args.mode == "run":
            run(*map(lambda p: Path(p).resolve(), args.arguments))
        else:
            fixture, binary, qualification, limit = args.arguments
            unit(Path(fixture), Path(binary), qualification, int(limit))
    except Exception:
        # Detailed subprocess output is retained only in the private proof Home.
        print("LA-04 resource proof failed; inspect the private proof logs.", file=sys.stderr)
        sys.exit(1)
