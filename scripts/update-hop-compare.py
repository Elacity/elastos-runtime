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

CLI CI fixtures use the same schema with mode `cli-install-update`. A package
has fixture.json, manifest.json and payload/. fixture.json supplies `root` and
`immutable` {reference, manifest: "manifest.json", sha256}. --root relocates
only this mode. ELASTOS_CI_FIXTURE_MANIFEST_SHA256 independently pins the
downloaded manifest. fixture.json's reference matches the pinned manifest's
preexisting public snapshot reference. ELASTOS_CI_FIXTURE_REFERENCE records
the separate outer github-actions:<owner/repository>:<artifact-id> retrieval
identity; an artifact ID is assigned after the immutable package is uploaded.
The manifest supplies schema, mode, reference, approval, proof_kind, source
{commit,tree}, channel, signer_did, platform, installer (a payload path), old
and new {version,source:{commit,tree}}, and files mapping every payload-relative
path (including the `payload/` prefix) to {bytes,sha256,mode,cid?}. The inventory
is closed: each payload file occurs once and symlinks/hard links are refused.

`proof_scope` defaults to production-positive: `publications` has old/new and
both selectors have empty refusals. The independently pinned operator package
owns real M1/M2 acceptance. CI generates a separate `ci-rehearsal` package with
old/new and wrong-signer-head, wrong-signer-release, tampered-binary, wrong-platform
and wrong-version. Each maps head, release, receipt, binary, components and
catalogue to inventoried paths. Receipt bytes contain last_head_cid and
last_release_cid. Metadata CIDs and signed envelope digests bind exact bytes.
Old is the newest published release, pinned by release CID and SHA-256 in
update-hop-previous-release.json (pin-previous-release moves the pin after a
publish; fetch-previous-release fetches its signed bytes into a SHA-256 cache).
Old keeps that release's Runtime, Home support and source installer; CI
republishes them under one disposable key, so this proves the published code and
data layout, not the maintainer key. build.rs compiles this source twice: as new,
the release after old, and as next, the release after new; the build receipt
binds sources, version environments, command and the binary hashes. Refusals claim
next's version and are offered to an installed new release, so this source's
update code refuses them. Through the pages, old's System refuses new (its support
changed, a known limit of old: #246) and keeps Home; old's CLI `update` installs
new; new's System refuses a tampered offer and applies next; data is written and
the hop is undone with `update --rollback-to` after a plain update to old is
refused. check-result is the CI verdict on result.json.
`selectors` uses m1-install/old and m2-discovery/new with the same refusal list.

`holder.files` maps Home-relative destinations to inventoried public payload
paths, including .local/bin/elastos, native components.json, bin/ipfs-provider,
bin/kubo and complete public CID storage under ipfs-repo/. Config and private
Kubo identity material are excluded; fixed `kubo init --profile=test` creates
the disposable repository before public blocks are copied.
`holder.content` maps each served CID to its exact inventoried payload path.
The holder starts through the fixed gateway command. One writer copies each
approved publication to the existing Publisher paths while it is stopped.
`consumer.files` maps data-relative paths to inventoried preservation/support
files. `preserve` has nonempty config, data and support lists of data-relative
paths. Additional named groups are permitted. CI also preserves its generated
identity. Package mappings refuse private identity keys.
Consumer host-process.lock is Runtime coordination, with exact offline upgrade
metadata and released ownership checked after commands and in snapshots. M2 apply
binds its recorded PID to the owned command; user data and support remain separate.
The operator owns real release signing and publication. Native Mac CI creates
and removes disposable signing keys through generate-ci-hop, using Runtime's
sign-payload command and offline Kubo content from the existing Mac build.
Each scope has its own frozen installer, holder, consumers and receipt.
Generated native CI packages pin the qualified localhost provider and complete
source Home bytes through fixture archive and native artifact descriptors. Setup
proves downloads from the isolated fixture publisher. Production release archives,
model inference and manual UX remain with their separate acceptance gates.
Qualified localhost capsule metadata retains its original archive checksum,
size and extraction paths. Its admitted archive and complete extracted file
inventory join the setup closure when that source descriptor requires them.
Generation requires that original archive through --localhost-metadata.
Successful M1 and refusal-seed installs run installed default setup over Carrier
before their installed metadata and support are checked. The fixture packages
the qualified source Home and native executables; setup must produce those exact
bytes before preservation sentinels are copied. A separate M1 Home starts
through `home --browser` and its signed retained controller. The observer checks
its live gateway generation, private attach, served Home bytes and owned shutdown.
This initial-start proof keeps the offline CLI snapshots and refusal checks intact.
Frozen installer defaults pin signer_did and leave
HEAD_CID blank. Atomic runtime ticket/node overrides select the local holder.
Run performs both selectors, retains private split output and data, and checks
all owned process groups and verified holder/consumer executable roots during
cleanup. Self-tests prove the observer only; installed proof needs real-runtime.

For an approved local Mac rehearsal, pass
--local-rehearsal local-rehearsal:<40-character-source-commit>:<fixture-name>
to build-ci-hop, generate-ci-hop, inspect and run. This selector requires the
clean source of this harness, absent CI/GITHUB_ACTIONS authority and an empty
artifact retrieval reference. Build and fixture receipts bind the same local
scope and reference. Local results keep command details in private split logs;
the safe result reports the failed step and exit code. The full install, Home,
Carrier update and refusal journey uses the same frozen support gates as CI.
"""

import argparse
import base64
import datetime
import errno
import ctypes
import fcntl
import gzip
import hashlib
import http.server
import io
import json
import os
from pathlib import Path
import platform
import pwd
import re
import shutil
import signal
import socket
import stat
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
import urllib.request
import urllib.parse


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
    if config.get("mode") == "cli-install-update":
        return cli_inspect(config)
    need(config.get("mode", "comparison") == "comparison", "unknown fixture mode")
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
    disk = shutil.disk_usage(root)
    growth = 2 * sum(paths[key].stat().st_size for key in ("binary", "components"))
    need(disk.free >= growth, "two update copies need more free disk space than the volume has")
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
    if config.get("mode") == "cli-install-update":
        return cli_run(config, output)
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
                    actor, args = role, ["update"]
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


CLI_MODE = "cli-install-update"
CLI_REFUSALS = ("wrong-signer-head", "wrong-signer-release", "tampered-binary", "wrong-platform", "wrong-version")
CLI_PHASES = ("old", "new", *CLI_REFUSALS)
# A cross-version fixture also publishes the release after new, which new's System applies.
CLI_SYSTEM_PHASE = "next"
CLI_DATA = "Library/Application Support/elastos"
CLI_PUBLISHER = "ElastOS/SystemServices/Publisher"
CLI_INSTALLED_METADATA = ("installation/release-head.json", "installation/release.json")
CI_XCODE_APPS = tuple("Xcode_" + version + ".app" for version in ("15.0.1", "15.1", "15.2", "15.3", "15.4", "16.1", "16.2"))


def cli_path(root, relative):
    need(isinstance(relative, str) and relative and str(Path(relative)) == relative
         and all(part not in (".", "..") for part in Path(relative).parts), "noncanonical fixture path")
    return inside(root, relative)


def cli_json(path):
    def unique(pairs):
        value = {}
        for key, item in pairs:
            need(key not in value, "duplicate fixture JSON field")
            value[key] = item
        return value
    return json.loads(path.read_text(), object_pairs_hook=unique)


def cli_environment(home_path):
    return {"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "HOME": str(home_path),
            "ELASTOS_CARRIER_NETWORK": "direct", "ELASTOS_QUIET_RUNTIME_NOTICES": "1"}


def cli_installer_metadata(installer, env):
    # The stamped defaults live inside the executable guard; sourcing skips it.
    # Read the two literal defaults without running any installer action.
    text = installer.read_text()
    values = []
    for variable in ("MAINTAINER_DID", "HEAD_CID"):
        need("ELASTOS_" + variable not in env, "fixture trust override is unavailable")
        matches = re.findall(r'^' + variable + r'="\$\{ELASTOS_' + variable +
                             r':-([^$\\"\r\n}]*)\}"$', text, re.M)
        need(len(matches) == 1, "frozen installer literal trust default unavailable")
        value = matches[0]
        values.append("" if value == "__" + variable + "__" else value)
    return values


def cli_signature(installer, envelope, domain, signer, env):
    command = 'source "$1"; verify_signature "$2" "$3" "$4"'
    proc = subprocess.run(["/bin/bash", "-c", command, "fixture", str(installer),
                           str(envelope), domain, signer], env=env, capture_output=True,
                          timeout=15, check=False)
    need(proc.returncode == 0, "fixture envelope signature invalid")


def cli_metadata_cid(cid, payload):
    def varint(value):
        result = bytearray()
        while value >= 128:
            result.append((value & 127) | 128)
            value >>= 7
        return bytes(result + bytes([value]))
    if cid.startswith("b"):
        encoded = cid[1:].upper()
        raw = base64.b32decode(encoded + "=" * (-len(encoded) % 8))
        need(raw[:1] == b"\x01", "fixture metadata CID version unsupported")
        raw = raw[1:]
    elif cid.startswith("Qm"):
        number = 0
        for char in cid:
            number = number * 58 + "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz".index(char)
        raw = b"\x70" + number.to_bytes((number.bit_length() + 7) // 8, "big")
    else:
        raise ValueError("fixture metadata CID unsupported")
    if raw[:1] == b"\x70":
        need(0 < len(payload) <= 256 * 1024, "fixture metadata exceeds single-block bound")
        unixfs = b"\x08\x02\x12" + varint(len(payload)) + payload + b"\x18" + varint(len(payload))
        payload = b"\x0a" + varint(len(unixfs)) + unixfs
    else:
        need(raw[:1] == b"\x55", "fixture metadata CID codec unsupported")
    need(raw[1:3] == b"\x12\x20" and raw[3:] == hashlib.sha256(payload).digest(),
         "fixture metadata CID bytes differ")


def cli_inventory(root, manifest):
    inventory = manifest["files"]
    need(isinstance(inventory, dict) and inventory, "immutable payload inventory required")
    actual = {str(path.relative_to(root)) for path in cli_path(root, "payload").rglob("*") if path.is_file()}
    need(actual == set(inventory), "immutable payload inventory is incomplete")
    for relative, binding in inventory.items():
        path = cli_path(root, relative)
        need(relative.startswith("payload/") and path.is_file(), "inventoried payload file required")
        need(path.stat().st_nlink == 1, "fixture payload uses a shared hard link")
        need(not any(part in ("identity", "private", "secrets") for part in path.relative_to(root).parts)
             and path.suffix not in (".key", ".pem", ".p12"), "package contains private key material")
        if path.suffix == ".json":
            def public(value):
                if isinstance(value, dict):
                    return all(not (key.lower().replace("_", "") in ("privkey", "privatekey", "secretkey", "signingkey", "devicekey") and item)
                               and public(item) for key, item in value.items())
                return not isinstance(value, list) or all(public(item) for item in value)
            need(public(cli_json(path)), "package JSON contains private key material")
        need(isinstance(binding["bytes"], int) and binding["bytes"] == path.stat().st_size,
             "fixture payload size differs")
        need(re.fullmatch(r"[0-9a-f]{64}", binding["sha256"]) and digest(path) == binding["sha256"],
             "fixture payload hash differs")
        need(binding["mode"] in (0o600, 0o644, 0o700, 0o755), "fixture installation mode invalid")
    need(not any(path.is_symlink() for path in (root / "payload").rglob("*")), "fixture payload contains a symlink")


def cli_model_fixture_capsule():
    return {"schema": "elastos.capsule/v1", "version": "0.1.0", "name": "model-update-hop-fixture",
            "role": "content", "type": "data", "entrypoint": "weights.gguf", "projections": ["content"],
            "model_content": {"format": "gguf", "quantization": "Q4_K_M", "engine": "llama.cpp",
                "consumer_interface": "elastos.provider.model", "consumer_interface_version": "0.1.0",
                "minimum_memory_mb": 8192, "license": {"spdx_id": "Apache-2.0", "path": "LICENSE"},
                "provenance": {"base_repository": "fixture/base", "base_revision": "a" * 40,
                    "base_license": {"spdx_id": "Apache-2.0", "path": "LICENSE.base"},
                    "quantized_repository": "fixture/quantized", "quantized_revision": "b" * 40,
                    "path": "PROVENANCE.md"}}}


def cli_model_fixture_object(manifest, mapping):
    entries = [{"path": path, "size": manifest["files"][relative]["bytes"],
                "sha256": manifest["files"][relative]["sha256"]} for path, relative in sorted(mapping.items())]
    closure = hashlib.sha256()
    for entry in entries:
        for value in (entry["path"], entry["sha256"], str(entry["size"])):
            closure.update(value.encode() + b"\0")
    return {"schema": "elastos.content.object.manifest/v1", "kind": "capsule",
            "content_digest": "sha256:" + closure.hexdigest(), "files": entries}


def cli_admit_model_fixture(root, manifest):
    fixture = manifest["model_fixture"]
    need(set(fixture) == {"package", "files"}
         and set(fixture["files"]) == {"capsule.json", "weights.gguf", "LICENSE", "LICENSE.base", "PROVENANCE.md"},
         "model fixture closure inventory differs")
    package = fixture["package"]
    mapping = fixture["files"]
    need(all(relative in manifest["files"] for relative in (package, *mapping.values())), "model fixture bytes absent")
    cid = manifest["files"][package]["cid"]
    encoded = cid[1:].upper()
    need(cid.startswith("b") and base64.b32decode(encoded + "=" * (-len(encoded) % 8))[:4] == b"\x01\x70\x12\x20",
         "model fixture package requires DAG-PB SHA-256 CIDv1")
    for relative in (package, *mapping.values()):
        binding = manifest["files"][relative]
        need(0 < binding["bytes"] <= 256 * 1024 and manifest["holder"]["content"].get(binding["cid"]) == relative,
             "model fixture holder CID mapping differs")
        cli_metadata_cid(binding["cid"], cli_path(root, relative).read_bytes())
    with tarfile.open(cli_path(root, package), mode="r:") as archive:
        members = archive.getmembers()
        need([member.name for member in members] == sorted(mapping)
             and all(member.isfile() and member.size == manifest["files"][mapping[member.name]]["bytes"] for member in members),
             "model fixture package inventory differs")
        for member in members:
            with archive.extractfile(member) as stream:
                need(stream.read() == cli_path(root, mapping[member.name]).read_bytes(), "model fixture package bytes differ")
    capsule = cli_model_fixture_capsule()
    need(cli_path(root, mapping["capsule.json"]).read_bytes() == json.dumps(capsule, sort_keys=True, separators=(",", ":")).encode(),
         "model fixture capsule bytes differ")
    entry = {"cid": cid, "capsule_manifest": capsule, "object_manifest": cli_model_fixture_object(manifest, mapping)}
    for publication in manifest["publications"].values():
        payload = cli_json(cli_path(root, publication["catalogue"]))["payload"]
        need(payload == {"schema": "elastos.model.catalog/v1", "published_at": 1, "expires_at": None, "entries": [entry]},
             "model fixture catalogue closure differs")


def cli_local_rehearsal(reference):
    match = re.fullmatch(r"local-rehearsal:([0-9a-f]{40}):([A-Za-z0-9][A-Za-z0-9_.-]{0,127})", reference)
    need(match is not None, "local rehearsal requires an exact source commit and fixture name")
    need(sys.platform == "darwin", "local rehearsal requires native Mac")
    need(not os.environ.get("CI") and not os.environ.get("GITHUB_ACTIONS"),
         "local rehearsal requires absent hosted CI authority")
    need(not os.environ.get("ELASTOS_CI_FIXTURE_REFERENCE"),
         "local rehearsal requires an empty artifact retrieval reference")
    need(os.environ.get("ELASTOS_CI_FIXTURE_SCOPE", "") in ("", "local-rehearsal"),
         "local rehearsal proof scope differs")
    repository = Path(__file__).resolve().parents[1]
    need(not subprocess.check_output(["git", "status", "--porcelain"], cwd=repository, text=True).strip(),
         "local rehearsal requires a clean admitted source")
    source = {key: subprocess.check_output(["git", "rev-parse", ref], cwd=repository, text=True).strip()
              for key, ref in (("commit", "HEAD"), ("tree", "HEAD^{tree}"))}
    need(source["commit"] == match[1], "local rehearsal source commit differs from its selector")
    return source


def cli_admit(config, local_rehearsal=None):
    need(config["schema"] == "elastos.update-hop.fixture/v1", "unknown fixture schema")
    need(config["mode"] == CLI_MODE, "unknown fixture mode")
    root = Path(config["root"])
    need(root.is_absolute() and root.is_dir() and root.resolve() == root,
         "physical prepared fixture root required")
    need(not any(part in ("tmp", "private", "target") for part in root.parts), "stable CI fixture root required")
    pin = config["immutable"]
    need(pin["manifest"] == "manifest.json", "fixed manifest.json required")
    manifest_path = cli_path(root, pin["manifest"])
    expected = os.environ.get("ELASTOS_CI_FIXTURE_MANIFEST_SHA256", "")
    retrieval = os.environ.get("ELASTOS_CI_FIXTURE_REFERENCE", "")
    need(expected and expected == pin["sha256"] == digest(manifest_path), "independently pinned manifest differs")
    reference = pin["reference"]
    need(isinstance(reference, str) and re.fullmatch(r"[A-Za-z][A-Za-z0-9+.-]*:[^\s]{1,1024}", reference), "public immutable snapshot reference required")
    need(not retrieval or re.fullmatch(r"github-actions:[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+:[1-9][0-9]*", retrieval), "outer artifact retrieval reference invalid")
    manifest = cli_json(manifest_path)
    need(manifest["schema"] == config["schema"] and manifest["mode"] == CLI_MODE
         and manifest["reference"] == reference, "fixture manifest identity differs")
    need(isinstance(manifest["approval"], str) and manifest["approval"].strip(), "fixture approval required")
    scope = manifest.get("proof_scope", "production-positive")
    if local_rehearsal is not None:
        need(scope == "local-rehearsal" and reference == local_rehearsal,
             "local rehearsal fixture scope or reference differs")
        need(manifest["source"] == cli_local_rehearsal(local_rehearsal), "local rehearsal source tree differs")
        need(manifest["new"]["source"] == manifest["source"] and ("previous" in manifest or manifest["old"]["source"] == manifest["source"]),
             "local compiled Runtime source differs")
    else:
        need(scope in ("production-positive", "ci-rehearsal") and
             scope == os.environ.get("ELASTOS_CI_FIXTURE_SCOPE", "production-positive"), "fixture proof scope differs")
        need(not reference.startswith("local-rehearsal:"), "local fixture requires its explicit selector")
    refusals = CLI_REFUSALS if scope in ("ci-rehearsal", "local-rehearsal") else ()
    need(manifest["proof_kind"] in ("real-runtime", "harness-self-test"), "unknown proof kind")
    need(os.environ.get("ELASTOS_CI_REQUIRE_REAL_RUNTIME") != "1" or manifest["proof_kind"] == "real-runtime", "hosted acceptance requires real-runtime fixture")
    need(sys.platform == "darwin" or manifest["proof_kind"] == "harness-self-test", "CLI installed proof requires native Mac")
    need(manifest["platform"] in ("aarch64-darwin", "x86_64-darwin"), "native Mac fixture platform required")
    if manifest["proof_kind"] == "real-runtime":
        need(manifest["platform"] == ("aarch64" if platform.machine() == "arm64" else "x86_64") + "-darwin",
             "fixture host architecture differs")
    for binding in (manifest["source"], manifest["old"]["source"], manifest["new"]["source"]):
        need(all(re.fullmatch(r"[0-9a-f]{40}", binding[key]) for key in ("commit", "tree")), "exact source commit/tree required")
    need(manifest["old"]["version"] != manifest["new"]["version"], "different old/new versions required")
    need(manifest["channel"] in ("canary", "jetson-test"), "isolated fixture channel required")
    signer = manifest["signer_did"]
    need(isinstance(signer, str) and signer.startswith("did:key:z"), "pinned fixture signer required")
    cli_inventory(root, manifest)
    installer = cli_path(root, manifest["installer"])
    need(manifest["installer"] in manifest["files"], "frozen installer missing from inventory")
    env = cli_environment(root)
    need(cli_installer_metadata(installer, env) == [signer, ""], "frozen installer signer or blank HEAD_CID differs")
    selectors = manifest["selectors"]
    need(set(selectors) == {"m1-install", "m2-discovery"}, "fixed CLI selectors required")
    for selector, positive in (("m1-install", "old"), ("m2-discovery", "new")):
        need(selectors[selector] == {"positive": positive, "refusals": list(refusals)}, "selector phase mapping differs")
    expected_phases = (set(CLI_PHASES) if refusals else {"old", "new"}) | ({CLI_SYSTEM_PHASE} if "previous" in manifest else set())
    need(set(manifest["publications"]) == expected_phases, "complete scoped publication set required")
    for phase, publication in manifest["publications"].items():
        need(set(publication) == {"head", "release", "receipt", "binary", "components", "catalogue"}, "publication snapshot incomplete")
        need(all(relative in manifest["files"] for relative in publication.values()), "publication bytes missing from inventory")
        head, release = (cli_json(cli_path(root, publication[key])) for key in ("head", "release"))
        receipt = cli_json(cli_path(root, publication["receipt"]))
        expected_version = cli_phase_version(manifest, phase)
        for key, envelope, domain in (("head", head, "elastos.release.head.v1"), ("release", release, "elastos.release.v1")):
            need(envelope["payload"]["version"] == expected_version and envelope["payload"]["channel"] == manifest["channel"], "publication version/channel differs")
            need(envelope["payload"]["schema"] == ("elastos.release.head/v1" if key == "head" else "elastos.release/v1"), "publication schema differs")
            wrong = phase == "wrong-signer-" + key
            need((envelope["signer_did"] != signer) == wrong, "publication signer phase differs")
            cli_signature(installer, cli_path(root, publication[key]), domain, envelope["signer_did"], env)
            cid = manifest["files"][publication[key]]["cid"]
            cli_metadata_cid(cid, cli_path(root, publication[key]).read_bytes())
            need(receipt["last_" + key + "_cid"] == cid, "publication receipt CID differs")
        release_binding = manifest["files"][publication["release"]]
        need(head["payload"]["latest_release_cid"] == release_binding["cid"]
             and head["payload"]["release_sha256"] == release_binding["sha256"], "head release binding differs")
        for key in ("binary", "components"):
            platforms = release["payload"]["platforms"]
            selected = manifest["platform"]
            if phase == "wrong-platform":
                need(selected not in platforms and len(platforms) == 1, "wrong-platform boundary differs")
                selected = next(iter(platforms))
                need(selected in ("aarch64-darwin", "x86_64-darwin"), "wrong-platform fixture is not a Mac release")
            declared = platforms[selected][key]
            actual = manifest["files"][publication[key]]
            need(declared["cid"] == actual["cid"] and declared["size"] == actual["bytes"], "release artifact CID/size differs")
            need((declared["sha256"] != actual["sha256"]) == (phase == "tampered-binary" and key == "binary"), "release artifact hash phase differs")
        catalogue = manifest["files"][publication["catalogue"]]
        components = cli_json(cli_path(root, publication["components"]))
        catalog_signer = cli_json(cli_path(root, publication["catalogue"]))["signer_did"]
        need(components["model_catalog"]["head_cid"] == catalogue["cid"]
             and catalog_signer in components["model_catalog"]["publisher_dids"], "catalogue trust binding differs")
        cli_metadata_cid(catalogue["cid"], cli_path(root, publication["catalogue"]).read_bytes())
        cli_signature(installer, cli_path(root, publication["catalogue"]), "elastos.model.catalog.v1", catalog_signer, env)
    old = manifest["publications"]["old"]
    new = manifest["publications"].get("new", old)
    need(manifest["files"][old["binary"]]["sha256"] != manifest["files"][new["binary"]]["sha256"], "different old/new binaries required")
    if local_rehearsal is None and manifest.get("build"):
        build = cli_json(cli_path(root, manifest["build"]))
        build_reference = build.get("reference", "")
        need(build.get("proof_scope") != "local-rehearsal" and isinstance(build_reference, str)
             and not build_reference.startswith("local-rehearsal:"), "local build requires its explicit selector")
    if refusals:
        wrong_version = manifest["publications"]["wrong-version"]
        need(manifest["files"][wrong_version["binary"]]["sha256"] == manifest["files"][old["binary"]]["sha256"], "wrong-version must retain the baseline Runtime bytes")
        need(manifest["build"] in manifest["files"], "compiled CI hop receipt required")
        build = cli_json(cli_path(root, manifest["build"]))
        need(build["schema"] == "elastos.update-hop.build/v1" and build["status"] == "passed"
             and build["cleanup"]["passed"] and build["source"] == manifest["source"]
             and build["command"] == ["cargo", "build", "--locked", "--release", "-p", "elastos-server", "--bin", "elastos"], "compiled CI hop provenance differs")
        need(build.get("proof_scope", "ci-rehearsal") == scope, "compiled hop proof scope differs")
        if local_rehearsal is not None:
            need(build.get("reference") == local_rehearsal, "compiled local hop reference differs")
        for name, publication in (("old", old), ("new", new)):
            need(build[name]["source"] == manifest[name]["source"] and build[name]["version"] == manifest[name]["version"]
                 and build[name]["sha256"] == manifest["files"][publication["binary"]]["sha256"], "compiled CI Runtime receipt differs")
        need(build["new"]["version_environment"] == manifest["new"]["version"], "next compiled version input differs")
    # A cross-version fixture publishes the previous release's own support as old.
    for key in ("components", "catalogue"):
        need(all(manifest["files"][publication[key]]["sha256"] == manifest["files"][new[key]]["sha256"]
                 for name, publication in manifest["publications"].items()
                 if not (key == "components" and name == "old" and "previous" in manifest)), "qualified support bytes changed")
    if "previous" in manifest:
        cli_admit_previous(root, manifest, env)
    if manifest["proof_kind"] == "real-runtime":
        for publication in (old, new):
            cli_macho(cli_path(root, publication["binary"]), manifest["platform"])
    for owner in ("holder", "consumer"):
        mapping = manifest[owner]["files"]
        need(isinstance(mapping, dict) and mapping, owner + " public file mapping required")
        for target, relative in mapping.items():
            cli_path(root, target)
            need(relative in manifest["files"] and "identity" not in Path(target).parts
                 and Path(target).name not in ("sources.json", "runtime-coords.json", "host-process.lock", "ipfs-coords.json"),
                 "fixture mapping contains private identity or live state")
            need("ipfs-repo" not in Path(target).parts or Path(target).parts[Path(target).parts.index("ipfs-repo") + 1:] and
                 Path(target).parts[Path(target).parts.index("ipfs-repo") + 1] in ("blocks", "datastore"), "holder repository mapping must contain public CID storage only")
            if owner == "consumer":
                need(target not in ("components.json", "model-catalog.json") and not target.startswith(CLI_PUBLISHER + "/"), "consumer mapping overwrites installer metadata")
    holder = manifest["holder"]
    for required in (".local/bin/elastos", CLI_DATA + "/components.json", CLI_DATA + "/bin/ipfs-provider", CLI_DATA + "/bin/kubo"):
        need(required in holder["files"], "holder provider/CID closure incomplete")
    need(any(target.startswith(CLI_DATA + "/ipfs-repo/blocks/") for target in holder["files"]), "holder public CID blocks required")
    need(all(relative in manifest["files"] and manifest["files"][relative].get("cid") == cid for cid, relative in holder["content"].items()), "holder CID mapping differs from inventory")
    for publication in manifest["publications"].values():
        for key in ("head", "release", "binary", "components", "catalogue"):
            relative = publication[key]
            mapped = holder["content"].get(manifest["files"][relative]["cid"])
            need(mapped in manifest["files"] and manifest["files"][mapped]["sha256"] == manifest["files"][relative]["sha256"], "holder CID content mapping incomplete")
    need(manifest["files"][holder["files"][".local/bin/elastos"]]["sha256"] in
         {manifest["files"][publication["binary"]]["sha256"] for publication in (old, new)}, "holder Runtime differs from qualified binaries")
    if "model_fixture" in manifest:
        cli_admit_model_fixture(root, manifest)
    need({"config", "data", "support"} <= set(manifest["preserve"]), "config/data/support preservation paths required")
    for group, paths in manifest["preserve"].items():
        need(isinstance(paths, list) and paths, "empty preservation group")
        for relative in paths:
            cli_path(root, relative)
            need(any(target == relative or target.startswith(relative + "/") for target in manifest["consumer"]["files"]), "preservation bytes missing")
    for view in cli_support_views(manifest):
        if "initial_home" in manifest:
            cli_admit_home_support(root, view)
        cli_admit_setup(root, view)
    need(scope not in ("ci-rehearsal", "local-rehearsal") or manifest["proof_kind"] != "real-runtime" or "initial_home" in manifest,
         "native CI rehearsal requires the installed Home startup fixture")
    need(scope not in ("ci-rehearsal", "local-rehearsal") or manifest["proof_kind"] != "real-runtime" or "model_fixture" in manifest,
         "native CI rehearsal requires the signed model catalogue fixture")
    need(scope not in ("ci-rehearsal", "local-rehearsal") or manifest["proof_kind"] != "real-runtime" or "previous" in manifest,
         "native CI rehearsal starts from the pinned published release")
    disk = shutil.disk_usage(root)
    growth = 8 * sum(value["bytes"] for value in manifest["files"].values())
    need(disk.free >= growth, "fixture copies need more free disk space than the volume has")
    return manifest


def cli_macho(path, release_platform):
    with path.open("rb") as stream:
        header = stream.read(4096)
    cpu = 0x0100000c if release_platform == "aarch64-darwin" else 0x01000007
    magic = header[:4]
    if magic in (b"\xcf\xfa\xed\xfe", b"\xfe\xed\xfa\xcf"):
        endian = "little" if magic[0] == 0xcf else "big"
        need(len(header) >= 32 and int.from_bytes(header[4:8], endian) == cpu
             and int.from_bytes(header[12:16], endian) == 2, "qualified Runtime Mach-O architecture differs")
    elif magic in (b"\xca\xfe\xba\xbe", b"\xca\xfe\xba\xbf"):
        count, width = int.from_bytes(header[4:8], "big"), 20 if magic[-1] == 0xbe else 32
        need(0 < count <= 32 and len(header) >= 8 + count * width and
             any(int.from_bytes(header[8 + index * width:12 + index * width], "big") == cpu for index in range(count)), "qualified Runtime universal architecture differs")
    else:
        raise ValueError("qualified Runtime requires a native Mach-O executable")


def cli_safe_error(error):
    if isinstance(error, (OSError, subprocess.SubprocessError)):
        return "fixture operation failed (" + type(error).__name__ + ")"
    if isinstance(error, KeyError):
        return "required fixture field missing"
    if isinstance(error, TypeError):
        return "fixture field type invalid"
    return str(error)


def cli_inspect(config, local_rehearsal=None):
    manifest = cli_admit(config, local_rehearsal)
    return {"mode": CLI_MODE, "status": "admitted", "proof_kind": manifest["proof_kind"],
            "proof_scope": manifest.get("proof_scope", "production-positive"),
            "manifest_sha256": config["immutable"]["sha256"], "source": manifest["source"],
            "reference": manifest["reference"], "retrieval_reference": os.environ.get("ELASTOS_CI_FIXTURE_REFERENCE", ""),
            "selectors": manifest["selectors"], "installed_proof": "pending real command results"}


def cli_reclaim_xcode(applications, protected, growth, measure, remove):
    """Internal helper permits bounded fixture roots in compile-free tests."""
    need(applications.is_dir() and applications.resolve() == applications and not applications.is_symlink(), "physical Xcode application root required")
    need(protected and all(path.is_dir() and path.resolve() == path and path.parent == applications
                          and path.name in CI_XCODE_APPS for path in protected), "selected Xcode ancestry differs")
    removed = []
    disk = measure()
    before = disk.free
    for name in CI_XCODE_APPS:
        if disk.free >= growth:
            break
        path = applications / name
        if not path.exists() or path.is_symlink() or path in protected:
            continue
        need(path.is_dir() and path.resolve() == path and path.parent == applications, "reclaim Xcode ancestry differs")
        allocated = remove(path)
        need(not path.exists() and all(retained.is_dir() for retained in protected), "Xcode reclaim or preservation failed")
        disk = measure()
        removed.append({"app": name, "allocated_bytes": allocated, "free_bytes_after": disk.free})
    return {"status": "ready" if disk.free >= growth else "unavailable",
            "retained": sorted(path.name for path in protected), "removed": removed,
            "free_bytes_before": before, "free_bytes_after": disk.free, "total_bytes": disk.total, "planned_growth_bytes": growth}


def cli_reclaim_android(runner_home, runner_uid, receipt, measure, remove):
    """Internal helper allows test homes; public CI uses one fixed SDK path."""
    if receipt["status"] == "ready":
        return receipt
    need(runner_home.is_absolute() and runner_home.is_dir() and runner_home.resolve() == runner_home
         and not runner_home.is_symlink() and runner_home.stat().st_uid == runner_uid,
         "Android SDK runner home ancestry or owner differs")
    sdk = runner_home / "Library/Android/sdk"
    if not sdk.exists() and not sdk.is_symlink():
        return receipt
    for path in (runner_home / "Library", runner_home / "Library/Android", sdk):
        need(path.is_dir() and not path.is_symlink() and path.resolve() == path
             and path.stat().st_uid == runner_uid and path.is_relative_to(runner_home),
             "Android SDK ancestry or owner differs")
    before = measure()
    if before.free >= receipt["planned_growth_bytes"]:
        receipt.update(status="ready", free_bytes_after=before.free, total_bytes=before.total)
        return receipt
    allocated = remove(sdk)
    need(not sdk.exists() and runner_home.is_dir() and (runner_home / "Library/Android").is_dir(), "Android SDK reclaim exceeded its owned directory")
    disk = measure()
    receipt["removed"].append({"tool": "runner Android SDK", "allocated_bytes": allocated, "free_bytes_before": before.free, "free_bytes_after": disk.free})
    receipt.update(status="ready" if disk.free >= receipt["planned_growth_bytes"] else "unavailable",
                   free_bytes_after=disk.free, total_bytes=disk.total)
    return receipt


def cli_prepare_ci_disk():
    need(os.environ.get("CI") == "true" and os.environ.get("GITHUB_ACTIONS") == "true"
         and os.environ.get("RUNNER_ENVIRONMENT") == "github-hosted" and sys.platform == "darwin"
         and pwd.getpwuid(os.geteuid()).pw_name == "runner", "capacity reclaim requires a disposable hosted Mac runner")
    checkout = Path(__file__).resolve().parents[1]
    workspace_root = Path("/Users/runner/work")
    runner_temp = Path(os.environ.get("RUNNER_TEMP", ""))
    need(checkout.is_relative_to(workspace_root) and Path.cwd().resolve() == checkout
         and runner_temp.is_absolute() and runner_temp.is_dir() and runner_temp.resolve() == runner_temp
         and runner_temp.is_relative_to(workspace_root), "hosted runner workspace ancestry differs")
    applications = Path("/Applications")
    runner_home = Path("/Users/runner")
    android_sdk = runner_home / "Library/Android/sdk"

    def query(argv):
        proc = subprocess.run(argv, capture_output=True, text=True, timeout=30, check=False)
        need(proc.returncode == 0 and proc.stdout.strip(), "selected Xcode query failed")
        return proc.stdout.strip()

    selected = query(["/usr/bin/xcode-select", "-p"])
    sdk = query(["/usr/bin/xcrun", "--show-sdk-path"])

    def app(path):
        need(Path(path).is_absolute(), "selected Xcode path must be absolute")
        physical = Path(path).resolve(strict=True)
        need(physical.is_relative_to(applications) and len(physical.relative_to(applications).parts) >= 1, "selected Xcode must belong to Applications")
        return applications / physical.relative_to(applications).parts[0]

    protected = {app(selected), app(sdk)}
    if os.environ.get("DEVELOPER_DIR"):
        protected.add(app(os.environ["DEVELOPER_DIR"]))
    alias = applications / "Xcode.app"
    if alias.exists():
        protected.add(app(str(alias)))

    def remove(path):
        need(path.resolve() == path and not path.is_symlink() and
             (path == android_sdk or path.parent == applications and path.name in CI_XCODE_APPS and path not in protected),
             "hosted tool reclaim target differs")
        need(query(["/usr/bin/xcode-select", "-p"]) == selected
             and query(["/usr/bin/xcrun", "--show-sdk-path"]) == sdk, "selected Xcode changed before reclaim")
        # du measures the allowlisted bundle; free space is measured again after
        # the fixed argv deletion. No shell expansion or user path is involved.
        size = query(["/usr/bin/du", "-sk", str(path)])
        need(re.fullmatch(r"[0-9]+\s+" + re.escape(str(path)), size) is not None, "hosted tool size measurement differs")
        # The root-owned remover has its own deadline. Runner credentials cannot
        # reliably signal root children through sudo on every hosted image.
        program = "import shutil,signal,sys; signal.alarm(180); shutil.rmtree(sys.argv[1])"
        proc = subprocess.run(["/usr/bin/sudo", "-n", "/usr/bin/python3", "-I", "-c", program, str(path)],
                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=185, check=False)
        need(proc.returncode == 0, "allowlisted hosted tool reclaim failed")
        return int(size.split()[0]) * 1024

    # The pinned published release (0.8.0-alpha.6) still keeps 15% of the volume free: below
    # it, its Home starts without the update controller ("Free disk space before updating")
    # and the journey cannot run. Drop this when the pin moves to a fixed-reserve release.
    growth = 20 * 1024**3 + cli_published_reserve(shutil.disk_usage(checkout).total)
    receipt = cli_reclaim_xcode(applications, protected, growth,
                                lambda: shutil.disk_usage(checkout), remove)
    receipt["image_inventory"] = "https://github.com/actions/runner-images/blob/macos-14-arm64/20260831.0302/images/macos/macos-14-arm64-Readme.md"
    try:
        receipt = cli_reclaim_android(runner_home, os.geteuid(), receipt,
                                       lambda: shutil.disk_usage(checkout), remove)
        need(query(["/usr/bin/xcode-select", "-p"]) == selected
             and query(["/usr/bin/xcrun", "--show-sdk-path"]) == sdk, "selected Xcode changed after reclaim")
        need(receipt["status"] == "ready",
             "hosted Mac capacity unavailable: free=" + str(receipt["free_bytes_after"]) + " total=" + str(receipt["total_bytes"]) + " planned_growth=" + str(receipt["planned_growth_bytes"]))
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        error.capacity = receipt
        raise
    return receipt


def cli_build_hop(root, runtime, previous, local_rehearsal=None):
    """Keep the pinned published Runtime as old; compile this source as the release after it
    (new, which old's CLI installs) and the one after that (next, which new's System applies)."""
    local_source = cli_local_rehearsal(local_rehearsal) if local_rehearsal is not None else None
    if local_source is None:
        need(os.environ.get("CI") == "true" and os.environ.get("GITHUB_ACTIONS") == "true"
             and sys.platform == "darwin", "disposable hop build requires native Mac CI")
    need(root.is_absolute() and not root.exists() and root.parent.resolve() == root.parent
         and not any(part in ("tmp", "private", "target") for part in root.parts), "fresh stable build root required")
    need(runtime.is_file() and not runtime.is_symlink(), "built Runtime input is unavailable")
    disk = shutil.disk_usage(root.parent)
    need(disk.free >= 4 * 1024**3, "hop rebuild needs more free disk space than the volume has")
    pin = cli_previous_pin()
    published = cli_previous_release(previous / "release.json", pin)
    release_platform = "aarch64-darwin" if platform.machine() == "arm64" else "x86_64-darwin"
    need(release_platform in published["platforms"], "previous release has no build for this Mac")
    published_binary = previous / "elastos"
    need(published_binary.is_file() and not published_binary.is_symlink()
         and digest(published_binary) == published["platforms"][release_platform]["binary"]["sha256"],
         "previous release Runtime differs from its signed hash")
    need(not subprocess.check_output(["git", "status", "--porcelain"], text=True).strip(), "hop build requires a clean admitted source")
    root.mkdir(mode=0o700)
    source = local_source or {key: subprocess.check_output(["git", "rev-parse", ref], text=True).strip()
              for key, ref in (("commit", "HEAD"), ("tree", "HEAD^{tree}"))}
    old = root / "elastos-old"
    shutil.copyfile(published_binary, old)
    old.chmod(0o755)
    cli_macho(old, release_platform)
    processes = CliProcesses(root)
    command = ["cargo", "build", "--locked", "--release", "-p", "elastos-server", "--bin", "elastos"]
    receipt = {"schema": "elastos.update-hop.build/v1", "source": source, "command": command, "status": "failed"}
    if local_rehearsal is not None:
        receipt.update(proof_scope="local-rehearsal", reference=local_rehearsal)
    try:
        reply = processes.command([str(old), "--version"], cli_environment(root), root, "old-version", timeout=15)
        need(reply["exit"] == 0 and not processes.text("old-version", "stderr")
             and processes.text("old-version") == "elastos " + pin["version"] + "\n", "previous release Runtime version differs")
        new_version = cli_next_version(pin["version"])
        receipt["old"] = {"version": pin["version"], "sha256": digest(old), "source": published["source"],
                          "release_sha256": pin["release_sha256"], "version_environment": None}
        for name, label, version in (("new", "build-next", new_version), ("next", "build-system", cli_next_version(new_version))):
            env = dict(os.environ)
            env["ELASTOS_RELEASE_VERSION"] = version
            built = processes.command(command, env, Path(__file__).resolve().parents[1] / "elastos", label, timeout=900)
            need(built["exit"] == 0, name + " Runtime build failed")
            binary = root / ("elastos-new" if name == "new" else "elastos-system")
            shutil.copyfile(runtime, binary)
            binary.chmod(0o755)
            cli_macho(binary, release_platform)
            reply = processes.command([str(binary), "--version"], cli_environment(root), root, name + "-version", timeout=15)
            need(reply["exit"] == 0 and processes.text(name + "-version") == "elastos " + version + "\n"
                 and not processes.text(name + "-version", "stderr")
                 and digest(binary) not in {value["sha256"] for value in receipt.values() if isinstance(value, dict) and "sha256" in value},
                 name + " Runtime exact version or bytes differ")
            receipt[name] = {"version": version, "sha256": digest(binary), "source": source, "version_environment": version}
        need(not subprocess.check_output(["git", "status", "--porcelain"], text=True).strip()
             and all(subprocess.check_output(["git", "rev-parse", ref], text=True).strip() == source[key]
                     for key, ref in (("commit", "HEAD"), ("tree", "HEAD^{tree}"))), "hop source changed during build")
        if local_rehearsal is not None:
            need(cli_local_rehearsal(local_rehearsal) == source, "local hop source changed during build")
        receipt["status"] = "passed"
    finally:
        receipt["cleanup"] = processes.cleanup()
        receipt["cleanup"]["passed"] = not receipt["cleanup"]["errors"]
        write(root / "build.json", receipt)
    need(receipt["cleanup"]["passed"], "hop builder cleanup failed")
    return receipt


def cli_home_activation_descriptor(registry, component_platform):
    descriptor = registry.get("external", {}).get("home")
    need(isinstance(descriptor, dict) and isinstance(descriptor.get("platforms"), dict),
         "Home activation binding is absent or invalid")
    platforms = descriptor["platforms"]
    aliases = {"darwin-arm64": ("aarch64-darwin",), "aarch64-darwin": ("darwin-arm64",)}.get(component_platform, ())
    selected = next((platforms[key] for key in (component_platform, *aliases, "*") if key in platforms), None)
    need(isinstance(selected, dict), "Home activation binding has no valid platform")
    install_path = selected.get("install_path")
    if install_path is None:
        install_path = descriptor.get("install_path")
    need(install_path == "capsules/home",
         "Home activation binding must resolve to capsules/home")
    return descriptor


def cli_qualified_home(support_home, component_platform, name="home"):
    """Admit an already built web capsule and the native Home provider."""
    support_home = support_home.resolve()
    registry = cli_json(cli_path(support_home, "components.json"))
    cli_home_activation_descriptor(registry, component_platform)
    native = cli_path(support_home, "bin/localhost-provider")
    info = native.lstat()
    need(stat.S_ISREG(info.st_mode) and info.st_nlink == 1 and info.st_mode & 0o111,
         "qualified localhost provider is unavailable")
    descriptor = registry["external"]["localhost-provider"]
    selected = descriptor["platforms"][component_platform]
    need(selected["checksum"] == "sha256:" + digest(native)
         and selected.get("install_path", descriptor.get("install_path")) == "bin/localhost-provider",
         "qualified localhost provider binding differs")
    entry = registry["capsules"][name]
    need(entry["install_path"] == "capsules/" + name and entry["entrypoint"] == "browser/index.html",
         "qualified Home entrypoint differs")
    capsule_root = cli_path(support_home, "capsules/" + name)
    paths = []
    for path in sorted(capsule_root.rglob("*")):
        relative = path.relative_to(support_home).as_posix()
        cli_path(support_home, relative)
        info = path.lstat()
        need(stat.S_ISDIR(info.st_mode) or stat.S_ISREG(info.st_mode) and info.st_nlink == 1,
             "qualified Home tree contains an unsupported file")
        if path.is_file():
            need(not set(path.relative_to(capsule_root).parts) & {"identity", "target", "node_modules", ".git"},
                 "qualified Home tree contains private or build state")
            paths.append(path)
    installed = cli_json(capsule_root / "capsule.json")
    need(installed["name"] == name and installed["entrypoint"] == entry["entrypoint"]
         and installed["execution"] == "web-projection", "qualified Home manifest differs")
    document = capsule_root / entry["entrypoint"]
    need(entry["entrypoint_sha256"] == "sha256:" + digest(document)
         and entry["entrypoint_size"] == document.stat().st_size, "qualified Home document binding differs")
    assets = [{"path": path.relative_to(capsule_root).as_posix(), "sha256": "sha256:" + digest(path),
               "size": path.stat().st_size} for path in paths if path.is_relative_to(capsule_root / "browser")]
    need(entry["browser_assets"] == assets and assets, "qualified Home browser asset closure differs")
    return entry, paths, descriptor


def cli_admit_home_support(root, manifest):
    fixture = manifest["initial_home"]
    need(set(fixture) == {"entrypoint", "files"}
         and fixture["entrypoint"] == "capsules/home/browser/index.html", "initial Home fixture differs")
    mapping = manifest["consumer"]["files"]
    native_paths = {"bin/" + name for name in CLI_NATIVE_SETUP}
    expected = set(cli_setup_files(manifest)) | {"fixture-tools/open"}
    need(fixture["files"] == sorted(expected) and native_paths <= expected
         and "fixture-tools/open" in expected and "capsules/home/capsule.json" in expected and fixture["entrypoint"] in expected,
         "initial Home support closure is incomplete")
    # Without a previous release, one support set persists across the update.
    need("previous" in manifest or all(any(target == path or target.startswith(path + "/") for path in manifest["preserve"]["support"])
                                       for target in native_paths), "initial Home native support preservation is incomplete")
    components = cli_json(cli_path(root, manifest["publications"][manifest.get("support_of", "old")]["components"]))
    platform_name = "darwin-arm64" if manifest["platform"] == "aarch64-darwin" else "darwin-amd64"
    cli_home_activation_descriptor(components, platform_name)
    entry = cli_capsule_entries(root, manifest, components)["home"]
    need(entry["install_path"] == "capsules/home" and entry["entrypoint"] == "browser/index.html",
         "signed Home entrypoint differs")
    document = manifest["files"][mapping[fixture["entrypoint"]]]
    need(entry["entrypoint_sha256"] == "sha256:" + document["sha256"] and entry["entrypoint_size"] == document["bytes"],
         "signed Home document differs from the inventory")
    installed = cli_json(cli_path(root, mapping["capsules/home/capsule.json"]))
    need(installed["name"] == "home" and installed["entrypoint"] == entry["entrypoint"]
         and installed["execution"] == "web-projection", "inventoried Home manifest differs")
    assets = [{"path": target.removeprefix("capsules/home/"), "sha256": "sha256:" + manifest["files"][mapping[target]]["sha256"],
               "size": manifest["files"][mapping[target]]["bytes"]}
              for target in sorted(expected) if target.startswith("capsules/home/browser/")]
    need(entry["browser_assets"] == assets, "signed Home browser closure differs from the inventory")
    for target in sorted(native_paths):
        native = manifest["files"][mapping[target]]
        descriptor = components["external"][Path(target).name]
        selected = descriptor["platforms"][platform_name]
        need(selected["checksum"] == "sha256:" + native["sha256"] and selected["cid"] == native["cid"]
             and selected["size"] == native["bytes"]
             and selected.get("install_path", descriptor.get("install_path")) == target
             and native["mode"] & 0o111, "signed native Home support differs from the inventory")
    opener = manifest["files"][mapping["fixture-tools/open"]]
    need(opener["mode"] == 0o700, "initial Home opener ownership mode differs")
    for target in expected:
        binding = manifest["files"][mapping[target]]
        relative = manifest["holder"]["content"].get(binding.get("cid"))
        need(relative in manifest["files"] and manifest["files"][relative]["sha256"] == binding["sha256"],
             "initial Home support CID closure differs")


CLI_WEB_CAPSULES = ("home", "home-gui", "system")
# Home-profile providers whose status must match the running Runtime; the update
# journey compares their startup warnings before and after the update.
CLI_PROTECTED_PROVIDERS = ("protected-content-protect-provider", "protected-content-decrypt-provider")
CLI_NATIVE_SETUP = ("ipfs-provider", "kubo", "localhost-provider", *CLI_PROTECTED_PROVIDERS)
CLI_SETUP_COMPONENTS = ("home", "ipfs-provider", "kubo", "localhost-provider", "home-gui", "system", *CLI_PROTECTED_PROVIDERS)
CLI_SETUP_ARTIFACTS = tuple(name + ".tar.gz" if name in CLI_WEB_CAPSULES else name for name in CLI_SETUP_COMPONENTS)
CLI_HOME_CACHE = (".elastos-cid", ".elastos-artifact-sha256")
CLI_LOCALHOST_METADATA = "localhost-provider-capsule-metadata.tar.gz"
CLI_RELEASE_SUPPORT = (*("bin/" + name for name in CLI_NATIVE_SETUP),
                       *("capsules/" + name + "/" for name in (*CLI_WEB_CAPSULES, "localhost-provider")))


def cli_localhost_metadata_info(descriptor, platform_name):
    metadata = descriptor.get("capsule_metadata")
    if metadata is None:
        return None
    need(isinstance(metadata, dict) and isinstance(metadata.get("platforms"), dict),
         "localhost metadata descriptor is invalid")
    aliases = {"darwin-arm64": ("aarch64-darwin",), "aarch64-darwin": ("darwin-arm64",)}.get(platform_name, ())
    selected = next((metadata["platforms"][key] for key in (platform_name, *aliases, "*")
                     if key in metadata["platforms"]), None)
    need(isinstance(selected, dict)
         and selected.get("install_path", metadata.get("install_path")) == "capsules/localhost-provider"
         and selected.get("extract_path") == "localhost-provider"
         and selected.get("release_path") == CLI_LOCALHOST_METADATA
         and re.fullmatch(r"sha256:[0-9a-f]{64}", selected.get("checksum", ""))
         and isinstance(selected.get("size"), int) and 0 < selected["size"] <= 16 * 1024**2
         and not any(selected.get(key) for key in ("url", "source", "strategy", "binary_path")),
         "localhost metadata archive binding differs")
    return selected


def cli_localhost_metadata_files(raw, selected):
    need(len(raw) == selected["size"] and "sha256:" + hashlib.sha256(raw).hexdigest() == selected["checksum"],
         "qualified localhost metadata archive checksum or size differs")
    files, seen, total = {}, set(), 0
    try:
        with gzip.GzipFile(fileobj=io.BytesIO(raw), mode="rb") as compressed:
            tar_bytes = compressed.read(16 * 1024**2 + 1)
        need(len(tar_bytes) <= 16 * 1024**2, "localhost metadata archive expansion exceeds bound")
        with tarfile.open(fileobj=io.BytesIO(tar_bytes), mode="r:") as archive:
            for member in archive:
                name = member.name.removeprefix("./").rstrip("/")
                path = Path(name)
                need(len(seen) < 1024 and name and not path.is_absolute() and path.as_posix() == name and ".." not in path.parts
                     and (name == "localhost-provider" or name.startswith("localhost-provider/"))
                     and name not in seen and (member.isdir() or member.isfile())
                     and member.mode in (0o600, 0o644, 0o700, 0o755), "unsafe localhost metadata archive member")
                seen.add(name)
                if member.isdir():
                    continue
                relative = path.relative_to("localhost-provider").as_posix()
                need(relative != "." and not set(path.parts) & {"identity", ".git", "target", "node_modules"}
                     and path.name not in CLI_HOME_CACHE and path.suffix not in (".key", ".pem", ".p12"),
                     "unsafe localhost metadata archive file")
                total += member.size
                need(0 <= member.size <= 16 * 1024**2 and total <= 16 * 1024**2,
                     "localhost metadata archive expansion exceeds bound")
                data = archive.extractfile(member).read()
                need(len(data) == member.size, "localhost metadata archive member is truncated")
                # Runtime's tar xzf inherits main's owner-only umask; fs::copy keeps that mode.
                files["capsules/localhost-provider/" + relative] = (data, member.mode & ~0o077)
    except (tarfile.TarError, OSError, EOFError) as error:
        raise ValueError("qualified localhost metadata archive is invalid") from error
    file_names = {"localhost-provider/" + target.removeprefix("capsules/localhost-provider/") for target in files}
    need(not any(parent.as_posix() in file_names for name in seen for parent in Path(name).parents),
         "localhost metadata archive file is used as a directory")
    capsule = files.get("capsules/localhost-provider/capsule.json")
    need(capsule is not None, "localhost metadata archive capsule manifest is absent")
    document = json.loads(capsule[0])
    need(isinstance(document, dict), "localhost metadata archive capsule manifest is invalid")
    icon = document.get("icon", "")
    need(isinstance(icon, str) and icon and not Path(icon).is_absolute()
         and Path(icon).as_posix() == icon and ".." not in Path(icon).parts
         and all("capsules/localhost-provider/" + icon + "/icon-" + str(size) + ".png" in files
                 for size in (32, 64, 128, 256)), "localhost metadata archive icon closure is incomplete")
    return files


def cli_setup_files(manifest):
    return {target: relative for target, relative in manifest["consumer"]["files"].items()
            if target.startswith(tuple("capsules/" + name + "/" for name in (*CLI_WEB_CAPSULES, "localhost-provider"))) or target in
            tuple("bin/" + name for name in CLI_NATIVE_SETUP)}


def cli_support_view(manifest, release):
    """One release's installed support: old is the pinned previous release, new is this
    source. Without a previous release every publication shares one support set."""
    if "previous" not in manifest:
        return manifest
    previous, setup_files = manifest["previous"], cli_setup_files(manifest)
    if release == "old":
        shared = {target: relative for target, relative in manifest["consumer"]["files"].items() if target not in setup_files}
        return {**manifest, "support_of": "old", "setup": previous["setup"], "consumer": {"files": {**shared, **previous["support"]}},
                "qualified_capsules": previous["qualified_capsules"], "publications": {"old": manifest["publications"]["old"]}}
    return {**manifest, "support_of": "new", "initial_home": {**manifest["initial_home"], "files": sorted({*setup_files, "fixture-tools/open"})},
            "publications": {name: value for name, value in manifest["publications"].items() if name != "old"}}


def cli_support_views(manifest):
    return [cli_support_view(manifest, release) for release in ("old", "new")] if "previous" in manifest else [manifest]


def cli_capsule_entries(root, manifest, components):
    """Qualified web capsule entries of one support set. A cross-version fixture publishes
    components.json as releases do, with an empty capsules map (release-platform-input.py),
    and keeps the qualified entries in an inventoried record beside it."""
    if "qualified_capsules" in manifest:
        need(components.get("capsules") == {}, "fixture components.json differs from a release's empty capsules map")
        return cli_json(cli_path(root, manifest["qualified_capsules"]))
    return components["capsules"]


def cli_admit_previous(root, manifest, env):
    """The old side must be the pinned published release, republished under the fixture signer."""
    previous = manifest["previous"]
    need(set(previous) == {"release", "published_components", "installer", "components", "setup", "support", "qualified_capsules"}
         and all(previous[key] in manifest["files"] for key in ("release", "published_components", "installer", "components", "qualified_capsules"))
         and manifest.get("qualified_capsules") in manifest["files"], "previous release fixture differs")
    pin = cli_previous_pin()
    release = cli_previous_release(cli_path(root, previous["release"]), pin)
    artifacts, old = release["platforms"][manifest["platform"]], manifest["publications"]["old"]
    need(manifest["old"]["version"] == pin["version"] and manifest["old"]["source"] == release["source"]
         and manifest["files"][old["binary"]]["sha256"] == artifacts["binary"]["sha256"]
         and manifest["files"][previous["published_components"]]["sha256"] == artifacts["components"]["sha256"]
         and old["components"] == previous["components"], "old publication is not the pinned release")
    need(manifest["new"]["version"] == cli_next_version(pin["version"]), "new publication is not the next release")
    # New's System applies the release after it: this source compiled again, with new's support.
    system, new = manifest["publications"][CLI_SYSTEM_PHASE], manifest["publications"]["new"]
    build = cli_json(cli_path(root, manifest["build"]))
    need(manifest[CLI_SYSTEM_PHASE]["version"] == cli_next_version(manifest["new"]["version"])
         and manifest[CLI_SYSTEM_PHASE]["source"] == manifest["new"]["source"]
         and build[CLI_SYSTEM_PHASE]["version"] == manifest[CLI_SYSTEM_PHASE]["version"]
         and build[CLI_SYSTEM_PHASE]["sha256"] == manifest["files"][system["binary"]]["sha256"]
         and manifest["files"][system["binary"]]["sha256"] not in
             {manifest["files"][manifest["publications"][name]["binary"]]["sha256"] for name in ("old", "new")}
         and system["components"] == new["components"], "System publication is not this source's following release")
    if manifest["proof_kind"] == "real-runtime":
        cli_macho(cli_path(root, system["binary"]), manifest["platform"])
    need(cli_installer_metadata(cli_path(root, previous["installer"]), env) == [manifest["signer_did"], ""],
         "previous installer signer or blank HEAD_CID differs")
    # Native support is published unpacked, so its signed hash binds the installed bytes;
    # fetch-previous-release checked the archives (Kubo, capsules) by their signed hashes.
    published = cli_json(cli_path(root, previous["published_components"]))
    platform_name = "darwin-arm64" if manifest["platform"] == "aarch64-darwin" else "darwin-amd64"
    for name in ("ipfs-provider", "localhost-provider", *CLI_PROTECTED_PROVIDERS):
        signed = cli_platform_entry(published["external"][name], platform_name)
        need(manifest["files"][previous["support"]["bin/" + name]]["sha256"] == signed["checksum"].removeprefix("sha256:"),
             "old native support differs from the published release")


def cli_home_archive(root, manifest, mapping, name="home"):
    output = io.BytesIO()
    with gzip.GzipFile(fileobj=output, mode="wb", filename="", mtime=0) as compressed:
        with tarfile.open(fileobj=compressed, mode="w", format=tarfile.USTAR_FORMAT) as archive:
            for target, relative in sorted(mapping.items()):
                if not target.startswith("capsules/" + name + "/"):
                    continue
                raw = cli_path(root, relative).read_bytes()
                binding = manifest["files"][relative]
                need(len(raw) == binding["bytes"] and hashlib.sha256(raw).hexdigest() == binding["sha256"],
                     "Home archive input differs from inventory")
                member = tarfile.TarInfo(target.removeprefix("capsules/"))
                member.size, member.mode, member.mtime = len(raw), binding["mode"], 0
                archive.addfile(member, io.BytesIO(raw))
    return output.getvalue()


def cli_admit_setup(root, manifest):
    setup = manifest.get("setup")
    if setup is None:
        need(manifest["proof_kind"] == "harness-self-test",
             "real Runtime fixture requires signed setup artifacts")
        return {}
    has_metadata = "localhost_metadata" in setup
    need(set(setup) == ({"artifacts", "localhost_metadata"} if has_metadata else {"artifacts"})
         and set(setup["artifacts"]) == set(CLI_SETUP_ARTIFACTS) | ({CLI_LOCALHOST_METADATA} if has_metadata else set()),
         "fixed setup artifact closure required")
    mapping = cli_setup_files(manifest)
    need(all("bin/" + name in mapping for name in CLI_SETUP_COMPONENTS if name not in CLI_WEB_CAPSULES)
         and all("capsules/" + name + "/browser/index.html" in mapping for name in CLI_WEB_CAPSULES),
         "setup output inventory is incomplete")
    need(not any(Path(target).name in CLI_HOME_CACHE for target in mapping),
         "setup cache metadata belongs to installed setup")
    platform_name = "darwin-arm64" if manifest["platform"] == "aarch64-darwin" else "darwin-amd64"
    for publication in manifest["publications"].values():
        components = cli_json(cli_path(root, publication["components"]))
        need(components.get("profiles", {}).get("home", {}).get("components") == list(CLI_SETUP_COMPONENTS),
             "fixed nonempty setup Home profile required")
        for name, artifact in zip(CLI_SETUP_COMPONENTS, CLI_SETUP_ARTIFACTS):
            relative = setup["artifacts"][artifact]
            need(relative in manifest["files"], "setup artifact missing from inventory")
            binding = manifest["files"][relative]
            component = components["external"][name]
            need(name == "localhost-provider" or "capsule_metadata" not in component,
                 "setup component has unmodeled capsule metadata")
            selected = component["platforms"][platform_name]
            target = "capsules/" + name if name in CLI_WEB_CAPSULES else "bin/" + name
            need(selected.get("release_path") == artifact and selected.get("install_path") == target
                 and not any(selected.get(key) for key in ("url", "source", "strategy"))
                 and selected.get("cid") == binding.get("cid")
                 and selected.get("checksum") == "sha256:" + binding["sha256"]
                 and selected.get("size") == binding["bytes"], "signed setup artifact binding differs")
            need(manifest["files"].get(manifest["holder"]["content"].get(binding["cid"]), {}).get("sha256") == binding["sha256"],
                 "setup holder artifact is missing")
            if name in CLI_WEB_CAPSULES:
                entry = cli_capsule_entries(root, manifest, components)[name]
                need(cli_path(root, relative).read_bytes() == cli_home_archive(root, manifest, mapping, name),
                     "Home setup archive differs from qualified bytes")
                need(selected.get("extract_path") == name
                     and entry["cid"] == binding["cid"] and entry["sha256"] == "sha256:" + binding["sha256"]
                     and entry["size"] == binding["bytes"], "Home setup package identity differs")
            else:
                need(relative == mapping[target] and binding["mode"] & 0o111
                     and not selected.get("extract_path"), "native setup artifact differs from output inventory")
        selected = cli_localhost_metadata_info(components["external"]["localhost-provider"], platform_name)
        need((selected is not None) == has_metadata, "localhost metadata setup closure differs")
        metadata_targets = {target for target in mapping if target.startswith("capsules/localhost-provider/")}
        if has_metadata:
            closure = setup["localhost_metadata"]
            need(set(closure) == {"artifact", "files"} and closure["artifact"] == CLI_LOCALHOST_METADATA,
                 "localhost metadata setup declaration differs")
            relative = setup["artifacts"][CLI_LOCALHOST_METADATA]
            binding = manifest["files"][relative]
            need(selected.get("cid") == binding.get("cid") and binding["sha256"] == selected["checksum"].removeprefix("sha256:")
                 and binding["bytes"] == selected["size"]
                 and manifest["files"].get(manifest["holder"]["content"].get(binding["cid"]), {}).get("sha256") == binding["sha256"],
                 "signed localhost metadata artifact differs")
            expected = cli_localhost_metadata_files(cli_path(root, relative).read_bytes(), selected)
            need(closure["files"] == sorted(expected) and metadata_targets == set(expected),
                 "localhost metadata output inventory is incomplete")
            for target, (raw, mode) in expected.items():
                output = manifest["files"][mapping[target]]
                need(output["bytes"] == len(raw) and output["sha256"] == hashlib.sha256(raw).hexdigest()
                     and output["mode"] == mode, "localhost metadata output binding differs")
        else:
            need(not metadata_targets, "localhost metadata output has no signed archive")
    return mapping


def cli_verify_setup_support(root, manifest, home_path):
    if "setup" not in manifest:
        need(manifest["proof_kind"] == "harness-self-test", "real Runtime setup closure is absent")
        return
    directory = home_path / CLI_DATA
    for target, relative in cli_setup_files(manifest).items():
        path, binding = cli_path(directory, target), manifest["files"][relative]
        need(path.exists() or path.is_symlink(), "installed setup support is missing: " + target)
        info = path.lstat()
        need(stat.S_ISREG(info.st_mode) and info.st_nlink == 1 and info.st_uid == os.geteuid()
             and digest(path) == binding["sha256"] and info.st_size == binding["bytes"]
             and stat.S_IMODE(info.st_mode) == binding["mode"],
             "installed setup support differs: " + target)
    components = cli_json(directory / "components.json")
    platform_name = "darwin-arm64" if manifest["platform"] == "aarch64-darwin" else "darwin-amd64"

    def cache_identity(name):
        # Runtime records a capsules-map entry when it has one, else the signed archive (as releases publish).
        entry = components["capsules"].get(name)
        if entry and entry.get("cid", "").strip():
            return entry["cid"], entry["sha256"]
        selected = cli_platform_entry(components["external"][name], platform_name)
        return selected["cid"], selected["checksum"]
    caches = [("capsules/" + name, cache_identity(name)) for name in CLI_WEB_CAPSULES]
    if "localhost_metadata" in manifest["setup"]:
        platform_name = "darwin-arm64" if manifest["platform"] == "aarch64-darwin" else "darwin-amd64"
        selected = cli_localhost_metadata_info(components["external"]["localhost-provider"], platform_name)
        caches.append(("capsules/localhost-provider", (selected["cid"], selected["checksum"])))
        folder = directory / "capsules/localhost-provider"
        actual = {path.relative_to(directory).as_posix() for path in folder.rglob("*") if not path.is_dir()}
        expected = {target for target in cli_setup_files(manifest) if target.startswith("capsules/localhost-provider/")}
        need(actual == expected | {"capsules/localhost-provider/" + name for name in CLI_HOME_CACHE},
             "installed localhost metadata output closure differs")
    for folder, values in caches:
        for name, expected in zip(CLI_HOME_CACHE, values):
            path = cli_path(directory, folder + "/" + name)
            info = path.lstat()
            regular = (stat.S_ISREG(info.st_mode) and info.st_nlink == 1 and info.st_uid == os.geteuid()
                       and stat.S_IMODE(info.st_mode) == 0o600)
            actual = path.read_text() if regular else None
            if regular and name == ".elastos-artifact-sha256":
                # Archive receipts are bare sha256 hex; Homes set up by earlier releases keep the prefix.
                actual, expected = actual.removeprefix("sha256:"), expected.removeprefix("sha256:")
            need(regular and actual == expected + "\n", "installed setup cache identity differs")


# The cross-version hop starts from the newest published release. The pin names
# its maintainer-signed release by CID and SHA-256; pin-previous-release moves it.
CLI_PREVIOUS_PIN = Path(__file__).with_name("update-hop-previous-release.json")
CLI_PREVIOUS_ORIGIN = "https://elastos.elacitylabs.com"
# The Home profile the journey installs, plus custody, which a release lists outside it (#238).
CLI_PREVIOUS_COMPONENTS = (*CLI_SETUP_COMPONENTS, "custody-provider")


def cli_published_reserve(total):
    """Bytes the pinned published release keeps free before it creates its update controller:
    15% of the volume (install_transaction.rs RESERVE_PERCENT at its source commit)."""
    return -(-total * 15 // 100)


def cli_next_version(version):
    """The next release: the last numeric prerelease field, else the patch, plus one."""
    match = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z]+(?:\.[0-9A-Za-z]+)*))?", version)
    need(match is not None, "release version is not a plain semantic version")
    if match[4] is None:
        return ".".join([match[1], match[2], str(int(match[3]) + 1)])
    fields = match[4].split(".")
    need(fields[-1].isdigit(), "prerelease version has no numeric field to advance")
    fields[-1] = str(int(fields[-1]) + 1)
    return ".".join(match.groups()[:3]) + "-" + ".".join(fields)


def cli_phase_version(manifest, phase):
    """Refusals are offered to an installed new release, so they claim the release after it."""
    if phase in ("old", "new"):
        return manifest[phase]["version"]
    return cli_next_version(manifest["new"]["version"])


def cli_previous_pin(path=None):
    pin = cli_json(path or CLI_PREVIOUS_PIN)
    need(set(pin) == {"schema", "version", "release_cid", "release_sha256", "signer_did"}
         and pin["schema"] == "elastos.update-hop.previous-release/v1"
         and re.fullmatch(r"[0-9a-f]{64}", pin["release_sha256"]) and pin["signer_did"].startswith("did:key:z"),
         "previous release pin differs")
    cli_next_version(pin["version"])
    return pin


def cli_previous_release(path, pin):
    """Admit the maintainer-signed release bytes the pin names."""
    raw = path.read_bytes()
    need(hashlib.sha256(raw).hexdigest() == pin["release_sha256"], "previous release bytes differ from the pin")
    cli_metadata_cid(pin["release_cid"], raw)
    release = cli_json(path)
    with tempfile.TemporaryDirectory() as scratch:
        cli_signature(Path(__file__).with_name("install.sh"), path, "elastos.release.v1", pin["signer_did"], cli_environment(Path(scratch)))
    payload = release["payload"]
    need(release["signer_did"] == pin["signer_did"] and payload["schema"] == "elastos.release/v1"
         and payload["version"] == pin["version"]
         and all(re.fullmatch(r"[0-9a-f]{40}", payload["source"][key]) for key in ("commit", "tree")),
         "previous release identity differs")
    return payload


def cli_origin_bytes(path, limit):
    with urllib.request.urlopen(CLI_PREVIOUS_ORIGIN + path, timeout=300) as response:
        raw = response.read(limit + 1)
    need(len(raw) <= limit, "previous release download exceeds its bound")
    return raw


def cli_pin_previous_release(path=None):
    """Pin the release the public seed serves now; run after each published release."""
    with tempfile.TemporaryDirectory() as directory:
        scratch = Path(directory)
        (scratch / "release-head.json").write_bytes(cli_origin_bytes("/release-head.json", 64 * 1024))
        head = cli_json(scratch / "release-head.json")
        cli_signature(Path(__file__).with_name("install.sh"), scratch / "release-head.json",
                      "elastos.release.head.v1", head["signer_did"], cli_environment(scratch))
        pin = {"schema": "elastos.update-hop.previous-release/v1", "version": head["payload"]["version"],
               "release_cid": head["payload"]["latest_release_cid"], "release_sha256": head["payload"]["release_sha256"],
               "signer_did": head["signer_did"]}
        (scratch / "release.json").write_bytes(cli_origin_bytes("/ipfs/" + pin["release_cid"], 1024 * 1024))
        cli_previous_release(scratch / "release.json", pin)
    write(path or CLI_PREVIOUS_PIN, pin)
    return pin


def cli_platform_entry(descriptor, platform_name):
    selected = next((descriptor["platforms"][key] for key in (platform_name, "*") if key in descriptor.get("platforms", {})), None)
    need(isinstance(selected, dict) and selected.get("cid") and selected.get("release_path")
         and re.fullmatch(r"sha256:[0-9a-f]{64}", selected.get("checksum", "")) and isinstance(selected.get("size"), int),
         "previous release component is not a signed release artifact")
    return selected


def cli_previous_members(archive, prefix, destination):
    """Extract the regular files under prefix with the safety rules of the setup archives."""
    extracted = 0
    with tarfile.open(archive, mode="r:gz") as stream:
        for member in stream:
            name = member.name.removeprefix("./").rstrip("/")
            if not (name == prefix or name.startswith(prefix + "/")):
                continue
            path = Path(name)
            need(not path.is_absolute() and path.as_posix() == name and ".." not in path.parts
                 and (member.isdir() or member.isfile()), "unsafe previous release archive member")
            if member.isdir():
                continue
            relative = path.name if name == prefix else path.relative_to(prefix).as_posix()
            need(not set(Path(relative).parts) & {"identity", ".git", "target", "node_modules"}
                 and Path(relative).name not in CLI_HOME_CACHE, "unsafe previous release archive file")
            target = cli_path(destination, relative)
            target.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            with stream.extractfile(member) as source, target.open("xb") as output:
                shutil.copyfileobj(source, output)
            need(target.stat().st_size == member.size, "previous release archive member is truncated")
            target.chmod(0o755 if member.mode & 0o111 else 0o600)
            extracted += 1
    need(extracted, "previous release archive is empty")


def cli_fetch_previous_release(cache, output, pin_path=None):
    """Fetch the pinned release's Runtime, installer source and Home support by signed
    hash, and lay the support out like a source Home so a fixture can publish it."""
    pin = cli_previous_pin(pin_path)
    need(cache.is_absolute() and output.is_absolute() and not output.exists() and output.parent.is_dir(),
         "fresh previous release output required")
    cache.mkdir(mode=0o700, parents=True, exist_ok=True)
    output.mkdir(mode=0o700)
    component_platform = "darwin-arm64" if platform.machine() == "arm64" else "darwin-amd64"
    release_platform = "aarch64-darwin" if platform.machine() == "arm64" else "x86_64-darwin"
    receipt = {"schema": "elastos.update-hop.previous-release-fetch/v1", "pin": pin, "downloaded": [], "cached": []}

    def fetch(cid, sha256, size, name):
        # The release metadata is bound by its pinned hash; signed artifacts also by size.
        cached = cache / sha256
        if cached.is_file() and not cached.is_symlink() and digest(cached) == sha256:
            receipt["cached"].append(name)
        else:
            raw = cli_origin_bytes("/ipfs/" + cid, size or 1024 * 1024)
            need(size in (None, len(raw)) and hashlib.sha256(raw).hexdigest() == sha256, name + " differs from its signed hash")
            partial = cache / (sha256 + ".partial")
            partial.write_bytes(raw)
            os.replace(partial, cached)
            receipt["downloaded"].append(name)
        target = cli_path(output, name)
        target.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        shutil.copyfile(cached, target)
        return target

    release = cli_previous_release(fetch(pin["release_cid"], pin["release_sha256"], None, "release.json"), pin)
    need(release_platform in release["platforms"], "previous release has no build for this Mac")
    artifacts = release["platforms"][release_platform]
    for key, name in (("binary", "elastos"), ("components", "components.json")):
        fetch(artifacts[key]["cid"], artifacts[key]["sha256"], artifacts[key]["size"], name)
    (output / "elastos").chmod(0o755)
    # A release signs its source; its installer is that source's install.sh.
    source, repository = release["source"], Path(__file__).resolve().parents[1]
    if subprocess.run(["git", "cat-file", "-e", source["commit"] + "^{commit}"], cwd=repository, capture_output=True).returncode:
        subprocess.run(["git", "fetch", "--quiet", "--no-tags", "--depth=1", "origin", source["commit"]],
                       cwd=repository, check=True, timeout=300)
    need(subprocess.check_output(["git", "rev-parse", source["commit"] + "^{tree}"], cwd=repository, text=True).strip() == source["tree"],
         "previous release source tree differs")
    installer = subprocess.check_output(["git", "show", source["commit"] + ":scripts/install.sh"], cwd=repository)
    need(installer.count(b"__MAINTAINER_DID__") == 1 and installer.count(b"__HEAD_CID__") == 1,
         "previous installer stamp placeholders differ")
    (output / "install.sh").write_bytes(installer)
    published = cli_json(output / "components.json")
    support = output / "support"
    (support / "bin").mkdir(mode=0o700, parents=True)
    external, capsules = {}, {}
    for name in CLI_PREVIOUS_COMPONENTS:
        descriptor = published["external"][name]
        selected = cli_platform_entry(descriptor, component_platform)
        archive = fetch(selected["cid"], selected["checksum"].removeprefix("sha256:"), selected["size"],
                        "artifacts/" + selected["release_path"])
        external[name] = descriptor
        if name in CLI_WEB_CAPSULES:
            need(selected.get("extract_path") == name, "previous capsule archive layout differs")
            folder = support / "capsules" / name
            cli_previous_members(archive, name, folder)
            manifest = cli_json(folder / "capsule.json")
            document = folder / manifest["entrypoint"]
            capsules[name] = {"cid": "", "sha256": "", "size": 0, "platforms": [component_platform],
                              "install_path": "capsules/" + name, "entrypoint": manifest["entrypoint"],
                              "entrypoint_sha256": "sha256:" + digest(document), "entrypoint_size": document.stat().st_size,
                              **{key: manifest.get(key) for key in ("runtime_abi", "execution", "bus_contract", "wit_world_sha256")},
                              "projections": manifest.get("projections") or [],
                              "browser_assets": [{"path": path.relative_to(folder).as_posix(), "sha256": "sha256:" + digest(path),
                                                  "size": path.stat().st_size}
                                                 for path in sorted(folder.rglob("*")) if path.is_file() and path.is_relative_to(folder / "browser")]}
        elif selected.get("extract_path"):
            cli_previous_members(archive, selected["extract_path"], support / "bin")
            os.replace(support / "bin" / Path(selected["extract_path"]).name, support / "bin" / name)
        else:
            shutil.copyfile(archive, support / "bin" / name)
            (support / "bin" / name).chmod(0o755)
        metadata = descriptor.get("capsule_metadata")
        if name == "localhost-provider" and metadata is not None:
            selected = cli_platform_entry(metadata, component_platform)
            need(selected["release_path"] == CLI_LOCALHOST_METADATA, "previous localhost metadata archive differs")
            os.replace(fetch(selected["cid"], selected["checksum"].removeprefix("sha256:"), selected["size"],
                             "artifacts/" + CLI_LOCALHOST_METADATA), output / CLI_LOCALHOST_METADATA)
    write(support / "components.json", {"schema": published["schema"], "capsules": capsules, "external": external,
                                        "profiles": {"home": {"components": list(CLI_SETUP_COMPONENTS)}}})
    shutil.rmtree(output / "artifacts")
    receipt["release"] = {"version": release["version"], "source": source, "binary_sha256": digest(output / "elastos"),
                          "components_sha256": digest(output / "components.json"), "installer_sha256": digest(output / "install.sh")}
    write(output / "previous.json", receipt)
    return receipt


def cli_generate_hop(root, runtime, next_runtime, system_runtime, build_receipt, support_home, previous, local_rehearsal=None, localhost_metadata=None):
    """Generate a disposable signed hop from the pinned published release to this source
    and its refusal set for the admitted scope."""
    local_source = cli_local_rehearsal(local_rehearsal) if local_rehearsal is not None else None
    if local_source is None:
        need(os.environ.get("CI") == "true" and os.environ.get("GITHUB_ACTIONS") == "true"
             and sys.platform == "darwin", "disposable refusal generation requires native Mac CI")
    need(root.is_absolute() and not root.exists() and root.parent.resolve() == root.parent
         and not any(part in ("tmp", "private", "target") for part in root.parts), "fresh stable refusal root required")
    component_platform = "darwin-arm64" if platform.machine() == "arm64" else "darwin-amd64"
    release_platform = "aarch64-darwin" if platform.machine() == "arm64" else "x86_64-darwin"

    def qualify(support, metadata_archive):
        """Admit one release's source-Home-shaped support before any signing."""
        support = support.resolve()
        capsules, paths = {}, []
        for name in CLI_WEB_CAPSULES:
            entry, found, localhost_descriptor = cli_qualified_home(support, component_platform, name)
            capsules[name] = entry
            paths.extend(found)
        info = cli_localhost_metadata_info(localhost_descriptor, component_platform)
        raw, files_ = None, {}
        if info is not None:
            need(metadata_archive is not None, "qualified localhost metadata archive input is required")
            metadata = metadata_archive.lstat()
            need(stat.S_ISREG(metadata.st_mode) and metadata.st_nlink == 1 and metadata.st_uid == os.geteuid()
                 and not metadata.st_mode & 0o022 and metadata.st_size == info["size"],
                 "qualified localhost metadata archive custody or size differs")
            raw = metadata_archive.read_bytes()
            files_ = cli_localhost_metadata_files(raw, info)
            if info.get("cid"):
                cli_metadata_cid(info["cid"], raw)
        else:
            need(metadata_archive is None, "localhost metadata archive input has no source descriptor")
        for path in (support / "bin" / name for name in CLI_NATIVE_SETUP):
            need(path.is_file() and not path.is_symlink(), "built refusal input is unavailable")
        size = (sum((support / "bin" / name).stat().st_size for name in CLI_NATIVE_SETUP) + sum(path.stat().st_size for path in paths)
                + sum(len(data) for data, _ in files_.values()) + len(raw or b""))
        return {"support": support, "qualified": cli_json(support / "components.json"), "capsules": capsules, "paths": paths,
                "localhost": localhost_descriptor, "metadata_info": info, "metadata_raw": raw, "metadata_files": files_, "bytes": size}

    sets = {"new": qualify(support_home, localhost_metadata)}
    pin = cli_previous_pin()
    published = cli_previous_release(previous / "release.json", pin)
    previous_metadata = previous / CLI_LOCALHOST_METADATA
    sets["old"] = qualify(previous / "support", previous_metadata if previous_metadata.exists() else None)
    for path in (runtime, next_runtime, system_runtime, build_receipt, previous / "install.sh", previous / "components.json"):
        need(path.is_file() and not path.is_symlink(), "built refusal input is unavailable")
    need(release_platform in published["platforms"]
         and digest(runtime) == published["platforms"][release_platform]["binary"]["sha256"]
         and digest(previous / "components.json") == published["platforms"][release_platform]["components"]["sha256"],
         "old Runtime or components differ from the pinned release")
    disk = shutil.disk_usage(root.parent)
    # Two Runtime copies plus their CID blocks, both releases' support, package copies
    # and eight isolated installed Homes fit within this conservative bound.
    growth = 12 * (2 * runtime.stat().st_size + next_runtime.stat().st_size + system_runtime.stat().st_size
                   + sum(value["bytes"] for value in sets.values()))
    need(disk.free >= growth, "refusal generation needs more free disk space than the volume has")
    root.mkdir(mode=0o700)
    scratch = root / "generator"
    scratch.mkdir(mode=0o700)
    source = local_source or {key: subprocess.check_output(["git", "rev-parse", ref], text=True).strip()
              for key, ref in (("commit", "HEAD"), ("tree", "HEAD^{tree}"))}
    manifest = {"schema": "elastos.update-hop.fixture/v1", "mode": CLI_MODE,
                "proof_scope": "local-rehearsal" if local_rehearsal is not None else "ci-rehearsal", "proof_kind": "real-runtime",
                "approval": "https://github.com/Elacity/elastos-runtime/issues/89#issuecomment-5972959202",
                "reference": local_rehearsal if local_rehearsal is not None else "ci-rehearsal:" + os.environ["GITHUB_RUN_ID"] + ":" + os.environ["GITHUB_RUN_ATTEMPT"],
                "source": source, "channel": "canary", "platform": release_platform,
                "files": {}, "publications": {}, "holder": {"files": {}, "content": {}},
                "selectors": {name: {"positive": positive, "refusals": list(CLI_REFUSALS)}
                              for name, positive in (("m1-install", "old"), ("m2-discovery", "new"))}}

    def add(name, content, mode=0o600):
        relative = "payload/" + name
        path = cli_path(root, relative)
        path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        if isinstance(content, Path):
            shutil.copyfile(content, path)
        elif isinstance(content, dict):
            write(path, content)
        else:
            path.write_bytes(content)
        path.chmod(mode)
        manifest["files"][relative] = {"bytes": path.stat().st_size, "sha256": digest(path), "mode": mode}
        return relative

    runtime_relative = add("elastos", runtime, 0o755)
    next_relative = add("elastos-next", next_runtime, 0o755)
    system_relative = add("elastos-system", system_runtime, 0o755)
    manifest["build"] = add("build.json", build_receipt)
    runtime_copy, next_copy, system_copy = root / runtime_relative, root / next_relative, root / system_relative
    kubo_relative = add("kubo", support_home / "bin/kubo", 0o755)
    env = cli_environment(scratch)
    env["IPFS_PATH"] = str(scratch / "ipfs-repo")

    def execute(argv, payload=None):
        proc = subprocess.run(argv, input=payload, capture_output=True, env=env, timeout=120, check=False)
        need(proc.returncode == 0, "disposable fixture command failed")
        return proc.stdout

    def sign(payload, domain, key):
        # This source signs; the published old Runtime only has to verify.
        canonical = json.dumps(payload, sort_keys=True, separators=(",", ":"), ensure_ascii=False, allow_nan=False).encode()
        reply = json.loads(execute([str(next_copy), "sign-payload", "--domain", domain, "--key", str(key)], canonical))
        return {"payload": payload, "signature": reply["signature"], "signer_did": reply["signer_did"]}

    def content(relative, raw=False):
        cid = execute([str(root / kubo_relative), "add", "--offline", "-Q", "--cid-version=1",
                       "--raw-leaves=" + str(raw).lower(), str(root / relative)]).decode().strip()
        manifest["files"][relative]["cid"] = cid
        manifest["holder"]["content"][cid] = relative
        return cid

    def package_support(prefix, current, catalogue):
        """Package one release's qualified support and the components.json that binds it."""
        support = current["support"]
        native = {name: add(prefix + name, support / "bin" / name, 0o755) for name in CLI_NATIVE_SETUP}
        mapping = {"bin/" + name: relative for name, relative in native.items()}
        for relative in native.values():
            content(relative)
        for path in current["paths"]:
            target = path.relative_to(support).as_posix()
            if path.name in CLI_HOME_CACHE:
                continue
            relative = add(prefix + "home-support/" + target, path)
            content(relative, raw=True)
            mapping[target] = relative
        capsules, archives, external = dict(current["capsules"]), {}, {}
        for name in CLI_WEB_CAPSULES:
            archive = add(prefix + name + ".tar.gz", cli_home_archive(root, manifest, mapping, name))
            content(archive)
            archives[name + ".tar.gz"] = archive
            binding = manifest["files"][archive]
            capsules[name] = {**capsules[name], "cid": binding["cid"],
                              "sha256": "sha256:" + binding["sha256"], "size": binding["bytes"]}
            external[name] = {"install_path": "capsules/" + name, "platforms": {component_platform: {
                "release_path": name + ".tar.gz", "extract_path": name, "install_path": "capsules/" + name,
                "cid": binding["cid"], "checksum": "sha256:" + binding["sha256"], "size": binding["bytes"]}}}
        for name in ("ipfs-provider", "kubo", *CLI_PROTECTED_PROVIDERS):
            relative = native[name]
            descriptor = current["qualified"]["external"][name]
            if name != "kubo":
                need(descriptor["platforms"][component_platform]["checksum"] == "sha256:" + manifest["files"][relative]["sha256"], "built support checksum differs")
            # Source-home verifies Kubo's archive pin. The disposable package pins
            # the installed executable bytes and exposes only local Carrier content.
            external[name] = {"install_path": "bin/" + name, "platforms": {component_platform: {
                "checksum": "sha256:" + manifest["files"][relative]["sha256"],
                "cid": manifest["files"][relative]["cid"],
                "size": manifest["files"][relative]["bytes"], "install_path": "bin/" + name,
                "release_path": name}}}
            if "provider_runtime" in descriptor:
                external[name]["provider_runtime"] = descriptor["provider_runtime"]
        # Preserve the qualified source descriptor and add the fixture CID pin.
        localhost = manifest["files"][native["localhost-provider"]]
        external["localhost-provider"] = {**current["localhost"], "platforms": {component_platform: {
            "install_path": "bin/localhost-provider", "checksum": "sha256:" + localhost["sha256"],
            "size": localhost["bytes"], "cid": localhost["cid"], "release_path": "localhost-provider"}}}
        setup = {"artifacts": {**archives, **native}}
        if current["metadata_info"] is not None:
            metadata_relative = add(prefix + CLI_LOCALHOST_METADATA, current["metadata_raw"])
            metadata_cid = content(metadata_relative)
            cli_metadata_cid(metadata_cid, current["metadata_raw"])
            external["localhost-provider"]["capsule_metadata"] = json.loads(json.dumps(current["localhost"]["capsule_metadata"]))
            cli_localhost_metadata_info(external["localhost-provider"], component_platform)["cid"] = metadata_cid
            for target, (raw, mode) in sorted(current["metadata_files"].items()):
                relative = add(prefix + "localhost-metadata/" + target, raw, mode)
                content(relative, raw=True)
                mapping[target] = relative
            setup["artifacts"][CLI_LOCALHOST_METADATA] = metadata_relative
            setup["localhost_metadata"] = {"artifact": CLI_LOCALHOST_METADATA, "files": sorted(current["metadata_files"])}
        # Like the release, list custody-provider outside the Home profile. Its
        # bytes are pinned but never served or installed: Home must start without it (#238).
        custody = support / "bin/custody-provider"
        custody_cid = execute([str(root / kubo_relative), "add", "--offline", "-Q", "--only-hash",
                               "--cid-version=1", str(custody)]).decode().strip()
        external["custody-provider"] = {**current["qualified"]["external"]["custody-provider"], "platforms": {component_platform: {
            "release_path": "custody-provider", "install_path": "bin/custody-provider", "cid": custody_cid,
            "checksum": "sha256:" + digest(custody), "size": custody.stat().st_size}}}
        # As releases publish (release-platform-input.py): an empty capsules map; web capsules
        # are external archives. A capsules entry makes `update` evict capsules/<name> after
        # refreshing it, which no released Home does.
        components = add(prefix + "components.json", {"schema": "elastos.components/v1", "capsules": {}, "external": external,
                         "profiles": {"home": {"description": "Qualified fixture Home", "components": list(CLI_SETUP_COMPONENTS)}},
                         "model_catalog": {"head_cid": manifest["files"][catalogue]["cid"], "publisher_dids": [manifest["signer_did"]]}})
        content(components)
        return {"components": components, "setup": setup, "support": mapping,
                "qualified_capsules": add(prefix + "qualified-capsules.json", capsules)}

    def stamp(installer):
        need(installer.count('__MAINTAINER_DID__') == 1 and installer.count('__HEAD_CID__') == 1, "installer stamp placeholders differ")
        return installer.replace('__MAINTAINER_DID__', manifest["signer_did"]).replace('__HEAD_CID__', '').encode()

    try:
        keys = [scratch / (name + ".key") for name in ("approved", "other")]
        for key in keys:
            key.write_text(os.urandom(32).hex())
            key.chmod(0o600)
        execute([str(root / kubo_relative), "init", "--profile=test"])
        capsule = cli_model_fixture_capsule()
        model_bytes = {"capsule.json": json.dumps(capsule, sort_keys=True, separators=(",", ":")).encode(),
                       "weights.gguf": b"GGUF synthetic fixture metadata only; no inference model.\n",
                       "LICENSE": b"Synthetic fixture bytes licensed under Apache-2.0.\n",
                       "LICENSE.base": b"Synthetic base fixture bytes licensed under Apache-2.0.\n",
                       "PROVENANCE.md": b"Owned local rehearsal fixture bytes only. No downloaded weights or inference claim.\n" if local_rehearsal is not None
                                        else b"Owned CI fixture bytes only. No downloaded weights or inference claim.\n"}
        model_mapping = {name: add("model-fixture/" + name, raw) for name, raw in sorted(model_bytes.items())}
        for relative in model_mapping.values():
            content(relative, raw=True)
        package_bytes = io.BytesIO()
        with tarfile.open(fileobj=package_bytes, mode="w", format=tarfile.USTAR_FORMAT) as archive:
            for name, raw in sorted(model_bytes.items()):
                member = tarfile.TarInfo(name)
                member.size, member.mode, member.mtime = len(raw), 0o600, 1
                archive.addfile(member, io.BytesIO(raw))
        package = add("model-fixture.tar", package_bytes.getvalue())
        package_cid = content(package)
        manifest["model_fixture"] = {"package": package, "files": model_mapping}
        catalogue_envelope = sign({"schema": "elastos.model.catalog/v1", "published_at": 1, "expires_at": None,
            "entries": [{"cid": package_cid, "capsule_manifest": capsule,
                         "object_manifest": cli_model_fixture_object(manifest, model_mapping)}]}, "elastos.model.catalog.v1", keys[0])
        manifest["signer_did"] = catalogue_envelope["signer_did"]
        catalogue = add("catalogue.json", catalogue_envelope)
        content(catalogue, raw=True)
        opener = add("home-opener", Path("/usr/bin/true"), 0o700)
        content(opener)
        new = package_support("", sets["new"], catalogue)
        old = package_support("previous/", sets["old"], catalogue)
        manifest["setup"], manifest["qualified_capsules"] = new["setup"], new["qualified_capsules"]
        # The old side is the published release: its Runtime, its Home support bytes and
        # the installer of its signed source, stamped with this fixture's disposable signer.
        manifest["previous"] = {"release": add("previous/release.json", previous / "release.json"),
                                "published_components": add("previous/published-components.json", previous / "components.json"),
                                "installer": add("previous/install.sh", stamp((previous / "install.sh").read_text())),
                                "components": old["components"], "setup": old["setup"], "support": old["support"],
                                "qualified_capsules": old["qualified_capsules"]}
        content(runtime_relative)
        content(next_relative)
        content(system_relative)
        old_version, new_version = pin["version"], cli_next_version(pin["version"])
        system_version = cli_next_version(new_version)
        need(execute([str(runtime_copy), "--version"]).decode() == "elastos " + old_version + "\n", "old Runtime must be the pinned release")
        need(execute([str(next_copy), "--version"]).decode() == "elastos " + new_version + "\n", "next Runtime must be a real compiled next release")
        need(execute([str(system_copy), "--version"]).decode() == "elastos " + system_version + "\n", "System Runtime must be a real compiled later release")
        build = cli_json(build_receipt)
        need(build["status"] == "passed" and build["cleanup"]["passed"] and build["source"] == source, "hop build receipt differs")
        need(build.get("proof_scope", "ci-rehearsal") == manifest["proof_scope"], "hop build proof scope differs")
        if local_rehearsal is not None:
            need(build.get("reference") == local_rehearsal, "local hop build reference differs")
        else:
            build_reference = build.get("reference", "")
            need(isinstance(build_reference, str) and not build_reference.startswith("local-rehearsal:"),
                 "local build requires its explicit selector")
        for name, relative, version, binary_source in (("old", runtime_relative, old_version, published["source"]),
                                                        ("new", next_relative, new_version, source),
                                                        ("next", system_relative, system_version, source)):
            need(build[name]["sha256"] == manifest["files"][relative]["sha256"] and build[name]["version"] == version
                 and build[name]["source"] == binary_source, "hop build Runtime binding differs")
            manifest[name] = {"version": version, "source": binary_source, "binary_sha256": build[name]["sha256"],
                              "version_environment": build[name]["version_environment"]}
        tampered = add("tampered-elastos", runtime_copy, 0o755)
        with (root / tampered).open("ab") as stream:
            stream.write(b"disposable refusal bytes")
        manifest["files"][tampered].update(bytes=(root / tampered).stat().st_size, sha256=digest(root / tampered))
        content(tampered)

        def binding(relative):
            item = manifest["files"][relative]
            return {"cid": item["cid"], "sha256": item["sha256"], "size": item["bytes"]}

        for phase in (*CLI_PHASES, CLI_SYSTEM_PHASE):
            binary_relative = {"tampered-binary": tampered, "old": runtime_relative, "wrong-version": runtime_relative,
                               CLI_SYSTEM_PHASE: system_relative}.get(phase, next_relative)
            binary_binding = binding(binary_relative)
            if phase == "tampered-binary":
                binary_binding["sha256"] = manifest["files"][runtime_relative]["sha256"]
            phase_platform = manifest["platform"]
            if phase == "wrong-platform":
                phase_platform = "x86_64-darwin" if phase_platform == "aarch64-darwin" else "aarch64-darwin"
            version = cli_phase_version(manifest, phase)
            components = old["components"] if phase == "old" else new["components"]
            # Home's System check requires the signed source; the change note lets
            # the installed journey tell each offered publication apart.
            release_payload = {"schema": "elastos.release/v1", "channel": "canary", "version": version,
                               "source": manifest["old" if phase == "old" else "new"]["source"], "changes": ["Fixture " + phase],
                               "platforms": {phase_platform: {"binary": binary_binding, "components": binding(components)}}}
            release = add(phase + "/release.json", sign(release_payload, "elastos.release.v1", keys[phase == "wrong-signer-release"]))
            content(release)
            head_payload = {"schema": "elastos.release.head/v1", "channel": "canary", "version": version,
                            "latest_release_cid": manifest["files"][release]["cid"], "release_sha256": digest(root / release), "updated_at": 1}
            head = add(phase + "/head.json", sign(head_payload, "elastos.release.head.v1", keys[phase == "wrong-signer-head"]))
            content(head)
            receipt = add(phase + "/receipt.json", {"last_head_cid": manifest["files"][head]["cid"], "last_release_cid": manifest["files"][release]["cid"]})
            manifest["publications"][phase] = {"head": head, "release": release, "receipt": receipt, "binary": binary_relative, "components": components, "catalogue": catalogue}
        manifest["installer"] = add("install.sh", stamp(Path(__file__).with_name("install.sh").read_text()))
        # This source serves the hop, as the seed serves releases it did not build.
        manifest["holder"]["files"] = {".local/bin/elastos": next_relative, CLI_DATA + "/components.json": new["components"],
                                       CLI_DATA + "/bin/ipfs-provider": new["support"]["bin/ipfs-provider"],
                                       CLI_DATA + "/bin/kubo": new["support"]["bin/kubo"]}
        for folder in ("blocks", "datastore"):
            for path in sorted((scratch / "ipfs-repo" / folder).rglob("*")):
                if path.is_file():
                    relative = str(path.relative_to(scratch / "ipfs-repo"))
                    manifest["holder"]["files"][CLI_DATA + "/ipfs-repo/" + relative] = add("repository/" + relative, path)
        manifest["consumer"] = {"files": {"config/fixture.json": add("consumer/config.json", {"owner": "isolated local rehearsal" if local_rehearsal is not None else "isolated CI refusal test"}),
                                           "state/sentinel": add("consumer/state", b"preserve user data"),
                                           "capsules/sentinel/data": add("consumer/support", b"preserve support"),
                                           "fixture-tools/open": opener, **new["support"]}}
        # Installs, setup and the initial Home start on the published release.
        manifest["initial_home"] = {"entrypoint": "capsules/home/browser/index.html", "files": sorted({*old["support"], "fixture-tools/open"})}
        # Release support changes with each update; cli_verify_setup_support checks it per release.
        manifest["preserve"] = {"config": ["config"], "data": ["state"], "support": ["capsules/sentinel", "fixture-tools"]}
        if local_rehearsal is not None:
            need(cli_local_rehearsal(local_rehearsal) == source, "local hop source changed during generation")
        write(root / "manifest.json", manifest)
        config = {"schema": manifest["schema"], "mode": CLI_MODE, "root": str(root),
                  "immutable": {"reference": manifest["reference"], "manifest": "manifest.json", "sha256": digest(root / "manifest.json")}}
        write(root / "fixture.json", config)
        return {"status": "generated", "proof_scope": manifest["proof_scope"], "reference": manifest["reference"], "manifest_sha256": config["immutable"]["sha256"],
                "source": source, "signer_did": manifest["signer_did"], "previous_release": pin["version"], "keys_removed": True}
    finally:
        shutil.rmtree(scratch)


def cli_census():
    proc = subprocess.run(["/bin/ps", "-axo", "pid=,ppid=,pgid=,command="],
                          capture_output=True, timeout=5, check=True)
    rows = []
    for line in proc.stdout.decode(errors="replace").splitlines():
        fields = line.strip().split(None, 3)
        if len(fields) == 4:
            rows.append({"pid": int(fields[0]), "parent": int(fields[1]),
                         "group": int(fields[2]), "command": fields[3]})
    return rows


class CliProcesses:
    def __init__(self, output):
        self.output, self.processes, self.streams, self.roots = output, [], [], {}

    def spawn(self, argv, env, cwd, label):
        stdout, stderr = ((self.output / (label + suffix)).open("wb") for suffix in (".stdout", ".stderr"))
        self.streams.extend((stdout, stderr))
        proc = subprocess.Popen(argv, env=env, cwd=cwd, stdin=subprocess.DEVNULL,
                                stdout=stdout, stderr=stderr, start_new_session=True)
        self.processes.append(proc)
        return proc

    def command(self, argv, env, cwd, label, timeout=90):
        started = time.monotonic()
        proc = self.spawn(argv, env, cwd, label)
        try:
            code = proc.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            os.killpg(proc.pid, signal.SIGKILL)
            proc.wait(timeout=5)
            code = 124
        return {"exit": code, "pid": proc.pid, "elapsed_ms": round((time.monotonic() - started) * 1000),
                "stdout_sha256": digest(self.output / (label + ".stdout")),
                "stderr_sha256": digest(self.output / (label + ".stderr"))}

    def text(self, label, stream="stdout"):
        return (self.output / (label + "." + stream)).read_text(errors="replace")

    def cleanup(self):
        errors, owned = [], []
        def census():
            live_groups = {proc.pid for proc in self.processes if proc.poll() is None}
            historical = {proc.pid for proc in self.processes}
            rows = cli_census()
            verified_groups = set(live_groups)
            verified_pids = set()
            for row in rows:
                root = next((path for path in self.roots if row["command"] == path or row["command"].startswith(path + " ")), None)
                if root:
                    need(digest(root) in self.roots[root], "cleanup executable hash differs")
                    verified_groups.add(row["group"])
                    verified_pids.add(row["pid"])
            matches = []
            for row in rows:
                if row["group"] in verified_groups or row["pid"] in verified_pids:
                    need(row["pid"] != os.getpid() and row["group"] != os.getpgrp(), "cleanup reached observer process")
                    matches.append(row)
                elif row["group"] in historical:
                    raise ValueError("historical process group has unverifiable ownership")
            return matches
        for sig in (signal.SIGTERM, signal.SIGKILL):
            try:
                observed = census()
            except (OSError, ValueError, subprocess.SubprocessError) as error:
                errors.append(cli_safe_error(error))
                observed = []
            for proc in reversed(self.processes):
                if proc.poll() is not None:
                    continue
                try:
                    os.killpg(proc.pid, sig)
                except ProcessLookupError:
                    pass
                except OSError as error:
                    errors.append(cli_safe_error(error))
            try:
                for row in observed:
                    current = next((item for item in cli_census() if item["pid"] == row["pid"]), None)
                    if current != row:
                        continue
                    try:
                        os.kill(row["pid"], sig)
                    except ProcessLookupError:
                        pass
                if sig == signal.SIGTERM:
                    time.sleep(0.2)
            except (OSError, ValueError, subprocess.SubprocessError) as error:
                errors.append(cli_safe_error(error))
        for proc in self.processes:
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                errors.append("owned process survived cleanup")
            owned.append({"pid": proc.pid, "exit": proc.returncode})
        for stream in self.streams:
            stream.close()
        try:
            remaining = census()
            if remaining:
                errors.append("owned fixture process remains")
        except (OSError, ValueError, subprocess.SubprocessError) as error:
            remaining = None
            errors.append(cli_safe_error(error))
        return {"owned_processes": owned, "remaining_pids": None if remaining is None else [row["pid"] for row in remaining], "errors": errors}


def cli_clean_output(processes, label):
    text = processes.text(label) + "\n" + processes.text(label, "stderr")
    return not re.search(r"\bERROR\b|ungraceful|endpoint[^\n]*(?:drop|closed unexpectedly)|connection lost", text, re.I)


def cli_success(processes, label, command):
    return command["exit"] == 0 and cli_clean_output(processes, label)


def cli_ci_stderr_detail(processes, manifest, label, description):
    # The immutable CI fixture uses disposable keys. Operator Runtime logs stay private.
    if manifest.get("proof_scope") != "ci-rehearsal" or not hasattr(processes, "output"):
        return ""
    stderr = processes.output / (label + ".stderr")
    try:
        with stderr.open("rb") as stream:
            stream.seek(max(0, stderr.stat().st_size - 4096))
            tail = stream.read(4096).decode(errors="replace")
        tail = cli_redact_text(tail, cli_known_secrets(processes.output)).strip()
        return "; CI " + description + " stderr: " + tail[-1024:] if tail else ""
    except OSError:
        return "; CI " + description + " diagnostic is unavailable"


CLI_SECRET_KEY = re.compile(r"secret|token|passkey|credential|password|cookie|ticket|authorization|private|signature", re.I)
CLI_SECRET_TEXT = (
    (re.compile(r"\x1b\[[0-9;]*m"), ""),
    (re.compile(r"(?i)\bbearer\s+\S+"), "Bearer [redacted]"),
    (re.compile(r"(?i)(\"?[\w.-]*(?:secret|token|passkey|credential|password|cookie|ticket|authorization|signature)[\w.-]*\"?\s*[:=]\s*)"
                r"(?!Bearer \[redacted\])(\"[^\"]*\"|'[^']*'|[^\s,;&}]+)"), r'\1"[redacted]"'))


def cli_redact(value):
    """Remove attach secrets, tokens, tickets and passkey material from diagnostic copies."""
    if isinstance(value, dict):
        return {key: "[redacted]" if CLI_SECRET_KEY.search(key) else cli_redact(item) for key, item in value.items()}
    if isinstance(value, list):
        return [cli_redact(item) for item in value]
    if isinstance(value, str):
        for pattern, replacement in CLI_SECRET_TEXT:
            value = pattern.sub(replacement, value)
    return value


def cli_known_secrets(output):
    """Secret values the run's Homes hold in their small JSON records (attach secrets, tokens),
    so a copied line that carries one is dropped even when nothing names it."""
    secrets = set()

    def collect(value):
        if isinstance(value, dict):
            for key, item in value.items():
                if CLI_SECRET_KEY.search(key) and isinstance(item, str) and len(item) >= 8:
                    secrets.add(item)
                collect(item)
        elif isinstance(value, list):
            for item in value:
                collect(item)

    for directory in sorted((output / "homes").glob("*/" + CLI_DATA)):
        for path in (*directory.glob("*.json"), *directory.glob("*/*.json")):
            try:
                if path.is_file() and not path.is_symlink() and path.stat().st_size <= 256 * 1024:
                    collect(json.loads(path.read_bytes()))
            except (OSError, ValueError):
                continue
    return secrets


def cli_redact_text(text, secrets):
    return "\n".join(cli_redact(line) for line in text.splitlines() if not any(secret in line for secret in secrets))


def cli_tail(path, secrets, lines=400):
    with path.open("rb") as stream:
        stream.seek(max(0, path.stat().st_size - 1024 * 1024))
        text = stream.read().decode(errors="replace")
    return cli_redact_text("\n".join(text.splitlines()[-lines:]), secrets) + "\n"


# Controller status fields copied into diagnostics; nothing else leaves the Home.
CLI_STATUS_FIELDS = ("phase", "message", "current_version", "new_version", "id", "generation", "host_pid")


def cli_listing(directory):
    if not directory.is_dir():
        return "absent: " + str(directory) + "\n"
    rows = []
    for path in sorted(directory.iterdir()):
        info = path.lstat()
        target = " -> " + os.readlink(path) if path.is_symlink() else ""
        rows.append(f"{stat.filemode(info.st_mode)} {info.st_nlink} {info.st_uid} {info.st_size} "
                    f"{datetime.datetime.fromtimestamp(info.st_mtime, datetime.timezone.utc).isoformat()} {path.name}{target}")
    return "\n".join(rows) + "\n"


def cli_failure_diagnostics(processes, manifest, homes, stage):
    """After a failure, keep redacted controller state and log tails in results/diagnostics:
    result.json keeps only hashes of private output. Disposable rehearsal fixtures only."""
    if manifest.get("proof_scope") not in ("ci-rehearsal", "local-rehearsal") or not hasattr(processes, "output"):
        return {}
    written, errors = [], []
    destination = processes.output / "diagnostics" / stage
    secrets = cli_known_secrets(processes.output)

    def keep(name, produce):
        try:
            text = produce()
            path = destination / name
            path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            path.write_text(text)
            path.chmod(0o600)
            written.append(str(path.relative_to(processes.output)))
        except (OSError, ValueError) as error:
            errors.append(name + ": " + cli_safe_error(error))

    try:
        # Controller, Home start, System browser, Undo and apply output of this run.
        for path in sorted(processes.output.glob("*.std*")):
            if re.search(r"home-start|home-system|^undo-|^cli-update|^m2-apply", path.name) and path.stat().st_size:
                keep("output/" + path.name, lambda path=path: cli_tail(path, secrets))
        for home in homes:
            directory = home / CLI_DATA
            if not directory.is_dir():
                continue
            prefix = home.name + "/"
            status = directory / "update-controller/status.json"
            if status.exists():
                keep(prefix + "update-controller-status.json", lambda status=status: json.dumps(
                    {key: value for key, value in cli_private_json(status).items() if key in CLI_STATUS_FIELDS},
                    indent=2, sort_keys=True) + "\n")
            logs = sorted(path for path in directory.rglob("*.log") if path.is_file() and "ipfs-repo" not in path.parts)[:30]
            for path in logs:
                keep(prefix + "logs/" + str(path.relative_to(directory)).replace("/", "__"), lambda path=path: cli_tail(path, secrets))
            keep(prefix + "ls-bin-installation.txt",
                 lambda directory=directory: "".join("$ ls -la " + name + "\n" + cli_listing(directory / name) for name in ("bin", "installation")))
    except (OSError, ValueError) as error:
        errors.append(cli_safe_error(error))
    return {"directory": str(destination.relative_to(processes.output)), "files": written, "errors": errors}


class CliBootstrap:
    """Serve admitted installer bytes; record and refuse every M2 HTTP fallback."""
    def __init__(self, root, manifest):
        self.root, self.manifest = root, manifest
        self.publication, self.enabled, self.fallback_requests, self.errors = None, False, 0, 0
        owner = self
        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass
            def setup(self):
                super().setup()
                self.connection.settimeout(5)
            def do_GET(self):
                if not owner.enabled:
                    owner.fallback_requests += 1
                    self.send_error(503)
                    return
                routes = {"/release-head.json": "head", "/release.json": "release",
                          "/artifacts/elastos-" + manifest["platform"]: "binary",
                          "/artifacts/components-" + manifest["platform"] + ".json": "components",
                          "/artifacts/model-catalog.json": "catalogue"}
                key = routes.get(self.path)
                if key is None:
                    self.send_error(404)
                    return
                relative = owner.publication[key]
                path, binding = cli_path(root, relative), manifest["files"][relative]
                if digest(path) != binding["sha256"]:
                    self.send_error(409)
                    return
                self.send_response(200)
                self.send_header("Content-Length", str(binding["bytes"]))
                self.end_headers()
                with path.open("rb") as stream:
                    shutil.copyfileobj(stream, self.wfile)
        class Server(http.server.ThreadingHTTPServer):
            def handle_error(self, *_):
                owner.errors += 1
        self.server = Server(("127.0.0.1", 0), Handler)
        self.server.daemon_threads = False
        self.url = "http://127.0.0.1:" + str(self.server.server_port)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)
        need(not self.thread.is_alive(), "installer bootstrap server survived cleanup")


def cli_copy_target(destination, relative):
    target = cli_path(destination, relative)
    parent = target.parent
    while True:
        if parent.exists():
            info = parent.lstat()
            need(stat.S_ISDIR(info.st_mode) and info.st_uid == os.geteuid(), "copy parent must be an owned directory")
        if parent == destination:
            break
        parent = parent.parent
    try:
        info = target.lstat()
    except FileNotFoundError:
        return target, None
    need(stat.S_ISREG(info.st_mode) and info.st_uid == os.geteuid(), "copy target must be an owned regular file")
    return target, (info.st_dev, info.st_ino, info.st_mode, info.st_uid)


def cli_copy(root, manifest, mapping, destination):
    for relative, source_relative in mapping.items():
        source_path = cli_path(root, source_relative)
        target, original = cli_copy_target(destination, relative)
        binding = manifest["files"][source_relative]
        need(stat.S_ISREG(source_path.lstat().st_mode), "copy source must be a regular file")
        need(digest(source_path) == binding["sha256"], "fixture changed before copy")
        target.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        need(cli_copy_target(destination, relative)[1] == original, "copy target changed before preparation")
        descriptor, temporary = tempfile.mkstemp(prefix=".elastos-fixture-copy-", dir=target.parent)
        temporary = Path(temporary)
        try:
            with os.fdopen(descriptor, "wb") as output, source_path.open("rb") as source:
                shutil.copyfileobj(source, output)
                os.fchmod(output.fileno(), binding["mode"])
            info = temporary.lstat()
            need(stat.S_ISREG(info.st_mode) and info.st_uid == os.geteuid()
                 and stat.S_IMODE(info.st_mode) == binding["mode"], "copied fixture ownership/mode differs")
            need(digest(temporary) == binding["sha256"], "copied fixture bytes differ")
            need(cli_copy_target(destination, relative)[1] == original, "copy target changed before replacement")
            os.replace(temporary, target)
        finally:
            temporary.unlink(missing_ok=True)


def cli_coordination(home_path, required=True):
    path, original = cli_copy_target(home_path, CLI_DATA + "/host-process.lock")
    if original is None:
        need(not required, "Runtime coordination file absent")
        return {"status": "absent"}
    need(stat.S_IMODE(original[2]) == 0o600, "Runtime coordination file mode differs")
    with path.open("rb") as stream:
        info = os.fstat(stream.fileno())
        need((info.st_dev, info.st_ino, info.st_mode, info.st_uid) == original, "Runtime coordination file changed during inspection")
        try:
            fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise ValueError("Runtime coordination lock is held") from error
        try:
            raw = stream.read(4097)
            need(len(raw) <= 4096, "Runtime coordination metadata exceeds its bound")
            def unique_fields(pairs):
                need(len(dict(pairs)) == len(pairs), "Runtime coordination metadata repeats a field")
                return dict(pairs)
            metadata = json.loads(raw, object_pairs_hook=unique_fields)
            need(isinstance(metadata, dict) and set(metadata) == {"pid", "role", "addr"}
                 and type(metadata["pid"]) is int and 0 < metadata["pid"] <= 0xffffffff
                 and metadata["role"] == "principal-root-upgrade" and metadata["addr"] == "offline",
                 "Runtime coordination metadata differs")
        finally:
            fcntl.flock(stream, fcntl.LOCK_UN)
    return {"status": "released", **metadata}


def cli_installed_metadata(home_path):
    directory = cli_path(home_path, CLI_DATA + "/installation")
    try:
        info = directory.lstat()
    except FileNotFoundError:
        return None
    need(stat.S_ISDIR(info.st_mode) and info.st_uid == os.geteuid()
         and stat.S_IMODE(info.st_mode) == 0o700, "installed metadata directory ownership/mode differs")
    need({path.name for path in directory.iterdir()} == {"release-head.json", "release.json"},
         "installed metadata inventory differs")
    snapshot = {}
    for relative in CLI_INSTALLED_METADATA:
        path = home_path / CLI_DATA / relative
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(descriptor, "rb") as stream:
            info = os.fstat(stream.fileno())
            need(stat.S_ISREG(info.st_mode) and info.st_uid == os.geteuid() and info.st_nlink == 1
                 and stat.S_IMODE(info.st_mode) == 0o600 and info.st_size <= 256 * 1024,
                 "installed metadata file ownership/mode/bound differs")
            raw = stream.read(256 * 1024 + 1)
            need(len(raw) <= 256 * 1024, "installed metadata file grew beyond its bound")
            snapshot[relative] = hashlib.sha256(raw).hexdigest()
    return snapshot


def cli_expected_installed_metadata(manifest, publication_name):
    publication = manifest["publications"][publication_name]
    return {relative: manifest["files"][publication[key]]["sha256"]
            for relative, key in zip(CLI_INSTALLED_METADATA, ("head", "release"))}


def cli_legacy_installed_metadata(home_path):
    snapshot = {}
    for relative in CLI_INSTALLED_METADATA:
        path, original = cli_copy_target(home_path, CLI_DATA + "/" + CLI_PUBLISHER + "/" + Path(relative).name)
        need(original is not None, "legacy installed metadata absent")
        info = path.lstat()
        need(info.st_nlink == 1 and info.st_mode & 0o7022 == 0 and info.st_size <= 256 * 1024,
             "legacy installed metadata ownership/mode/bound differs")
        snapshot[relative] = digest(path)
    return snapshot


def cli_state(manifest, home_path):
    directory = home_path / CLI_DATA
    coordination = cli_coordination(home_path)
    sources = cli_json(directory / "sources.json")
    need(sources["default_source"] == "default" and len(sources["sources"]) == 1, "one installer-owned source required")
    return {"binary": digest(home_path / ".local/bin/elastos"),
            "components": digest(directory / "components.json"),
            "catalogue": digest(directory / "model-catalog.json"), "sources": sources, "coordination": coordination,
            "installed_metadata": cli_installed_metadata(home_path),
            # Release support changes with the release; cli_verify_setup_support checks it per release.
            "data": {relative: binding for relative, binding in (files(directory) or {}).items()
                     if relative not in ("sources.json", "components.json", "model-catalog.json", "host-process.lock", *CLI_INSTALLED_METADATA)
                     and not relative.startswith(("backups/principal-root-upgrade-", *CLI_RELEASE_SUPPORT))},
            "preserved": {key: {relative: files(cli_path(directory, relative)) for relative in paths}
                          for key, paths in manifest["preserve"].items()}}


def cli_refusal(case, command, stdout, stderr, unchanged):
    boundary = {"tampered-binary": "SHA-256", "wrong-platform": "platform", "wrong-version": "version"}.get(case, "sign")
    # An unavailable holder, invalid CID or an unrelated failure is not a refusal proof.
    if case == "tampered-binary":
        evidence = "SHA-256 mismatch" in stderr and "Downloading binary" in stdout and "Downloading components" not in stdout and "Binary verified" not in stdout
    elif case == "wrong-platform":
        evidence = ("No release available for platform:" in stderr or "No binary CID for platform" in stderr) and "Downloading binary" not in stdout
    elif case == "wrong-version":
        evidence = ("Downloaded binary version mismatch" in stderr or "Installed binary version mismatch" in stderr) and "Downloading binary" in stdout
    else:
        evidence = ("Signer DID mismatch" in stderr or "Envelope signer differs from the pinned maintainer DID" in stderr)
        release_reached = "Fetching release:" in stdout or "Verifying release signature" in stdout
        evidence = evidence and release_reached == (case == "wrong-signer-release") and "Downloading binary" not in stdout
    return {"status": "passed" if command["exit"] not in (0, 124) and evidence and unchanged else "failed",
            "boundary": boundary, "preserved": unchanged, **command}


def cli_holder_node_id(did):
    """Decode canonical did:key spelling to Carrier key bytes; Runtime validates keys."""
    error = "holder DID requires one canonical Ed25519 did:key encoding"
    alphabet = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
    need(isinstance(did, str) and len(did) == 56 and did.startswith("did:key:z6Mk")
         and all(char in alphabet for char in did[9:]), error)
    number = 0
    for char in did[9:]:
        number = number * 58 + alphabet.index(char)
    raw = number.to_bytes((number.bit_length() + 7) // 8, "big")
    need(len(raw) == 34 and raw[:2] == b"\xed\x01", error)
    return raw[2:].hex()


def cli_holder_bootstrap(current, node_id):
    need(isinstance(current, dict) and current.get("schema") == "elastos.carrier.bootstrap/v1"
         and current.get("transport") == "carrier" and current.get("role") == "publisher"
         and current.get("node_id") == node_id, "holder transport identity differs")
    ticket = current.get("ticket")
    need(isinstance(ticket, str) and 0 < len(ticket) <= 65536
         and re.fullmatch(r"[a-zA-Z2-7]+", ticket) and len(ticket) % 8 in (0, 2, 4, 5, 7),
         "holder ticket encoding invalid")
    # Runtime owns ticket decoding and authenticated Carrier connection checks.


def cli_process_identity(pid):
    """Use the same kernel birth identity as the Runtime controller."""
    need(type(pid) is int and 0 < pid <= 0x7fffffff, "invalid fixture process identity")
    if sys.platform == "darwin":
        class BsdInfo(ctypes.Structure):
            # Darwin sys/proc_info.h, PROC_PIDTBSDINFO (flavor 3).
            _fields_ = [(name, ctypes.c_uint32) for name in (
                "flags", "status", "xstatus", "pid", "parent", "uid", "gid", "ruid", "rgid", "svuid", "svgid", "reserved")]
            _fields_ += [("comm", ctypes.c_char * 16), ("name", ctypes.c_char * 32)]
            _fields_ += [(name, ctypes.c_uint32) for name in ("nfiles", "group", "jobc", "tdev", "tpgid")]
            _fields_ += [("nice", ctypes.c_int32), ("seconds", ctypes.c_uint64), ("microseconds", ctypes.c_uint64)]
        library = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
        library.proc_pidinfo.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_uint64, ctypes.c_void_p, ctypes.c_int]
        library.proc_pidinfo.restype = ctypes.c_int
        info = BsdInfo()
        ctypes.set_errno(0)
        count = library.proc_pidinfo(pid, 3, 0, ctypes.byref(info), ctypes.sizeof(info))
        if count == 0 and ctypes.get_errno() == errno.ESRCH:  # Other kernel failures retain refusal.
            return None
        need(count == ctypes.sizeof(info) and info.pid == pid and info.status != 5,
             "fixture kernel process identity is incomplete")
        return {"pid": pid, "parent": info.parent, "group": info.group,
                "start": f"macos:{info.seconds}:{info.microseconds}"}
    need(sys.platform.startswith("linux"), "fixture process identity requires Darwin or Linux")
    path = Path(f"/proc/{pid}/stat")
    try:
        value = path.read_text()
    except FileNotFoundError:
        return None
    fields = value[value.rindex(")") + 2:].split()
    need(len(fields) >= 20 and fields[0] != "Z", "fixture kernel process identity is incomplete")
    boot = Path("/proc/sys/kernel/random/boot_id").read_text().strip()
    return {"pid": pid, "parent": int(fields[1]), "group": int(fields[2]), "start": "linux:" + boot + ":" + fields[19]}


def cli_process_executable(pid):
    if sys.platform == "darwin":
        library = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
        library.proc_pidpath.argtypes = [ctypes.c_int, ctypes.c_void_p, ctypes.c_uint32]
        library.proc_pidpath.restype = ctypes.c_int
        buffer = ctypes.create_string_buffer(4096)
        count = library.proc_pidpath(pid, buffer, len(buffer))
        need(0 < count < len(buffer), "fixture executable kernel path is incomplete")
        return os.fsdecode(buffer.value)
    need(sys.platform.startswith("linux"), "fixture executable identity requires Darwin or Linux")
    return os.readlink(f"/proc/{pid}/exe")


def cli_private_json(path, limit=256 * 1024):
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(descriptor, "rb") as stream:
        info = os.fstat(stream.fileno())
        need(stat.S_ISREG(info.st_mode) and info.st_uid == os.geteuid() and info.st_nlink == 1
             and stat.S_IMODE(info.st_mode) == 0o600 and info.st_size <= limit, "private Home record ownership or bound differs")
        raw = stream.read(limit + 1)
        need(len(raw) <= limit, "private Home record grew beyond its bound")
        def unique(pairs):
            need(len(dict(pairs)) == len(pairs), "private Home record repeats a field")
            return dict(pairs)
        return json.loads(raw, object_pairs_hook=unique)


def cli_home_base(value, public=False):
    url = urllib.parse.urlsplit(value)
    need(url.scheme == "http" and url.hostname in ("localhost", "127.0.0.1", "::1") and url.port
         and not url.username and not url.password and not url.query and not url.fragment,
         "Home readiness address is not exact loopback HTTP")
    need(url.path == ("/home/" if public else ""), "Home readiness address has an unexpected path")
    need(not public or value in ("http://localhost:8090/home/", "http://127.0.0.1:8090/home/", "http://[::1]:8090/home/"),
         "Home readiness listener differs")
    return url


def cli_home_response(url, limit, payload=None, token=None):
    class RefuseRedirect(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, *args):
            raise ValueError("Home readiness response redirected")
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), RefuseRedirect())
    headers = {"Content-Type": "application/json"} if payload is not None else {}
    if token is not None:
        headers["Authorization"] = "Bearer " + token
    request = urllib.request.Request(url, data=None if payload is None else json.dumps(payload).encode(), headers=headers)
    with opener.open(request, timeout=2) as response:
        need(response.status == 200, "Home readiness response refused")
        raw = response.read(limit + 1)
        need(len(raw) <= limit, "Home readiness response exceeds its bound")
        return raw


def cli_home_snapshot(manifest, home_path):
    directory = home_path / CLI_DATA
    return {"binary": digest(home_path / ".local/bin/elastos"), "components": digest(directory / "components.json"),
            "catalogue": digest(directory / "model-catalog.json"), "sources": cli_json(directory / "sources.json"),
            "identity": digest(directory / "identity/device.key"),
            "installed_metadata": cli_installed_metadata(home_path),
            "publisher": files(directory / CLI_PUBLISHER),
            "preserved": {key: {relative: files(cli_path(directory, relative)) for relative in paths}
                          for key, paths in manifest["preserve"].items()}}


def cli_observe_initial_home(processes, manifest, home_path, process, status, release="old"):
    directory = home_path / CLI_DATA
    expected = manifest["files"][manifest["publications"][release]["binary"]]["sha256"]
    controller = directory / "update-controller/runtime"
    receipt_path = directory / "update-controller/receipt.json"
    receipt = cli_private_json(receipt_path)
    need(receipt["schema"] == "elastos.update-controller/v1" and receipt["data_dir"] == str(directory)
         and receipt["binary"] == str(home_path / ".local/bin/elastos") and receipt["controller"] == str(controller)
         and receipt["controller_sha256"] == expected and digest(controller) == expected,
         "installed controller receipt or Runtime hash differs")
    info = controller.lstat()
    need(stat.S_ISREG(info.st_mode) and info.st_uid == os.geteuid() and info.st_nlink == 1
         and stat.S_IMODE(info.st_mode) == 0o700, "installed controller ownership differs")
    signed = base64.b64decode(receipt["signed_controller_release"], validate=True)
    need(hashlib.sha256(signed).hexdigest() == manifest["files"][manifest["publications"][release]["release"]]["sha256"]
         and receipt["trusted_source"] == source_for_home(home_path), "controller signed release or trust differs")
    launch = receipt["launch"]
    need([base64.b64decode(value, validate=True) for value in launch["args"]] == [b"home", b"--browser"]
         and base64.b64decode(launch["cwd"], validate=True) == os.fsencode(home_path)
         and hashlib.sha256(json.dumps({key: launch[key] for key in ("args", "environment", "cwd")}, separators=(",", ":")).encode()).hexdigest() == receipt["launch_sha256"],
         "controller launch receipt differs")
    controller_identity = cli_process_identity(process.pid)
    host_identity = cli_process_identity(status["host_pid"])
    need(controller_identity is not None and controller_identity["group"] == process.pid
         and status["controller_pid"] == process.pid and status["controller_start"] == controller_identity["start"]
         and host_identity is not None and host_identity["parent"] == process.pid
         and host_identity["group"] == host_identity["pid"]
         and cli_process_executable(process.pid) == str(controller)
         and cli_process_executable(host_identity["pid"]) == str(home_path / ".local/bin/elastos"),
         "controller or owned Home child identity differs")
    generation = status["generation"]
    need(re.fullmatch(r"[0-9a-f]{32}", generation) and status["phase"] == "ready"
         and status["current_version"] == manifest[release]["version"] and status["id"] is None and status["new_version"] is None,
         "controller initial readiness status differs")
    coords_path = directory / "gateway-runtime-coords.json"
    coords = cli_private_json(coords_path)
    need(coords["runtime_kind"] == "gateway" and coords["pid"] == host_identity["pid"]
         and coords["generation"] == generation and coords["binary_sha256"] == expected
         and re.fullmatch(r"[0-9a-f]{64}", coords["attach_secret"]), "live gateway coordinates differ")
    cli_home_base(coords["home_url"], public=True)
    cli_home_base(coords["api_url"])
    host_lock = cli_private_json(directory / "host-process.lock", 4096)
    need(host_lock["pid"] == host_identity["pid"] and host_lock["role"] == "gateway"
         and host_lock["addr"] == "localhost:8090" and host_lock["generation"] == generation
         and lock_state(directory / "host-process.lock") == "held"
         and lock_state(directory / "update-controller/controller.lock") == "held", "live Home ownership lock differs")
    attached = json.loads(cli_home_response(coords["api_url"] + "/api/auth/attach", 16 * 1024,
                                           {"secret": coords["attach_secret"], "scope": "client"}))
    token = attached.get("token")
    need(isinstance(token, str) and token and attached.get("session_type") == "capsule", "private Home attach refused")
    health_reply = json.loads(cli_home_response(coords["api_url"] + "/api/health", 4096, token=token))
    need(health_reply["version"] == manifest[release]["version"], "authenticated Home health version differs")
    document = directory / manifest["initial_home"]["entrypoint"]
    served = cli_home_response(coords["home_url"], 2 * 1024 * 1024)
    need(hashlib.sha256(served).hexdigest() == digest(document), "served Home differs from its installed capsule")
    need(cli_process_identity(process.pid) == controller_identity and cli_process_identity(host_identity["pid"]) == host_identity
         and cli_private_json(coords_path) == coords and process.poll() is None, "Home generation changed during readiness proof")
    processes.roots[str(controller)] = {expected}
    return {"status": "passed", "proof_scope": "installed-initial-home", "support_scope": "frozen-source-home",
            "controller": controller_identity, "host": host_identity, "generation": generation,
            "controller_receipt_sha256": digest(receipt_path), "controller_sha256": expected,
            "gateway_coords_sha256": digest(coords_path), "home_sha256": hashlib.sha256(served).hexdigest(),
            "home_url": coords["home_url"], "api_url": coords["api_url"], "authenticated_health": True}


def source_for_home(home_path):
    sources = cli_json(home_path / CLI_DATA / "sources.json")
    return next(value for value in sources["sources"] if value["name"] == sources["default_source"])


def cli_port_released(value):
    url = urllib.parse.urlsplit(value)
    for family, kind, protocol, _, address in socket.getaddrinfo(url.hostname, url.port, type=socket.SOCK_STREAM):
        with socket.socket(family, kind, protocol) as probe:
            probe.settimeout(.2)
            need(probe.connect_ex(address) != 0, "initial Home listener survives shutdown")


def cli_initial_home(processes, manifest, home_path, evidence=None, publish=None, root=None):
    """Run the installed entrypoint; Runtime alone admits and owns its child. A real-runtime
    fixture then runs the supported update path through the pages (cli_home_system_journey)
    and undoes it, starting Home again on the published release."""
    before = cli_home_snapshot(manifest, home_path)
    expected_metadata = cli_expected_installed_metadata(manifest, "old")
    need(before["installed_metadata"] in (None, expected_metadata), "initial Home installed metadata differs from signed old release")
    if before["installed_metadata"] is None:
        need(cli_legacy_installed_metadata(home_path) == expected_metadata, "initial Home legacy metadata differs from signed old release")
    # Initial Home may migrate the frozen installer's legacy signed pair once.
    before["installed_metadata"] = expected_metadata
    directory = home_path / CLI_DATA
    for value in ("http://localhost:8090/home/", "http://127.0.0.1:8090/home/"):
        cli_port_released(value)
    env = cli_environment(home_path)
    env["ELASTOS_CARRIER_MDNS"] = "0"
    opener = cli_path(directory, "fixture-tools/open")
    binding = manifest["files"][manifest["consumer"]["files"]["fixture-tools/open"]]
    info = opener.lstat()
    need(digest(opener) == binding["sha256"] and stat.S_ISREG(info.st_mode) and info.st_nlink == 1
         and info.st_uid == os.geteuid() and stat.S_IMODE(info.st_mode) == 0o700, "initial Home opener differs")
    env["PATH"] = str(opener.parent) + ":" + env["PATH"]
    processes.roots[str(opener)] = {binding["sha256"]}
    if "previous" in manifest:
        disk = shutil.disk_usage(directory)
        need(disk.free >= cli_published_reserve(disk.total) + (home_path / ".local/bin/elastos").stat().st_size,
             "runner free disk " + str(disk.free) + " of " + str(disk.total) + " is below the published release's 15% "
             "reserve, so its Home would start without the update controller")
    process, proof, failure, cleanup_failure, exit_code, identities = None, None, None, None, None, []
    evidence = {} if evidence is None else evidence
    binary = home_path / ".local/bin/elastos"
    stages = processes.output / "home-system" if hasattr(processes, "output") else None

    def head(release):
        return manifest["files"][manifest["publications"][release]["head"]]["cid"]

    def expect(release):
        publication = manifest["publications"][release]
        before.update(binary=manifest["files"][publication["binary"]]["sha256"],
                      components=manifest["files"][publication["components"]]["sha256"],
                      installed_metadata=cli_expected_installed_metadata(manifest, release))
        before["sources"]["sources"][0].update(installed_version=manifest[release]["version"], head_cid=head(release))

    def command(label, args):
        return processes.command([str(binary), *args], cli_environment(home_path), home_path, label, timeout=300)

    def start(label, release):
        nonlocal process
        process = processes.spawn([str(binary), "home", "--browser"], env, home_path, label)
        deadline = time.monotonic() + 150
        while process.poll() is None and time.monotonic() < deadline:
            status_path = directory / "update-controller/status.json"
            if status_path.exists():
                status = cli_private_json(status_path)
                if status.get("phase") == "ready" and status.get("controller_pid") == process.pid:
                    ready = cli_observe_initial_home(processes, manifest, home_path, process, status, release)
                    identities.extend((ready["controller"], ready["host"]))
                    return ready
            time.sleep(.2)
        raise ValueError("installed Home did not reach controller readiness")

    def stop():
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGTERM)
        return process.wait(timeout=35)

    def released():
        for identity in identities:
            need(cli_process_identity(identity["pid"]) is None, "initial Home process survives shutdown")
        groups = {identity["group"] for identity in identities}
        need(not any(row["group"] in groups for row in cli_census()), "initial Home owned process group survives shutdown")
        need(not (directory / "gateway-runtime-coords.json").exists()
             and lock_state(directory / "host-process.lock") == "released"
             and lock_state(directory / "update-controller/controller.lock") == "released",
             "initial Home coordinates or ownership survive shutdown")
        need(not any((directory / "gateway-owned-runtimes").rglob("*.json")), "initial Home owned child record survives shutdown")
        for value in (proof["home_url"], "http://127.0.0.1:8090/home/", proof["api_url"]):
            cli_port_released(value)

    def restart(name, label, release):
        """Start Home on the installed release; the browser reopens it when it sees this file."""
        ready = start(label, release)
        write(stages / ("restart-" + name + ".json"), {"version": manifest[release]["version"], "generation": ready["generation"]})
        return {key: ready[key] for key in ("generation", "controller_sha256", "authenticated_health")}

    def update_with_cli(running_host):
        """The supported path from the published release: stop Home (including the Home
        process its System kept running), run its own `elastos update` over Carrier, start
        Home on the new release."""
        identities.append(running_host)
        need(stop() == 0, "Home did not stop cleanly before the CLI update")
        released()
        need(cli_home_snapshot(manifest, home_path) == before, "Home stop changed the installation or user data")
        reply = command("cli-update", ["update", "--yes"])
        need(cli_success(processes, "cli-update", reply) and "Discovery: Carrier" in processes.text("cli-update"),
             "the published release's CLI did not update to the new release; command exit " + str(reply["exit"])
             + cli_ci_stderr_detail(processes, manifest, "cli-update", "CLI update"))
        version = command("cli-update-version", ["--version"])
        need(version["exit"] == 0 and processes.text("cli-update-version") == "elastos " + manifest["new"]["version"] + "\n",
             "the CLI update did not install the new Runtime")
        expect("new")
        need(cli_home_snapshot(manifest, home_path) == before, "the CLI update changed user data or installed another release")
        cli_verify_setup_support(root, cli_support_view(manifest, "new"), home_path)
        return {"status": "passed", "update": reply, "readiness": restart("cli-update", "cli-update-home-start", "new")}

    def undo(browser):
        """Data written on the updated release survives an explicit CLI Undo; a plain update
        to the older release is refused; Home starts again on it with the same account."""
        written = cli_path(directory, "state/written-after-update")
        written.write_text(os.urandom(16).hex() + "\n")
        written.chmod(0o600)
        before["preserved"] = cli_home_snapshot(manifest, home_path)["preserved"]
        need(stop() == 0, "Home did not stop cleanly before Undo")
        released()
        need(cli_home_snapshot(manifest, home_path) == before, "Home stop changed the updated installation or user data")
        # The publisher now offers the older release, as an accidental downgrade would.
        publish("old")
        refused = command("undo-plain-update", ["update", "--yes"])
        need(refused["exit"] != 0 and "older than installed release" in processes.text("undo-plain-update", "stderr")
             + processes.text("undo-plain-update") and cli_home_snapshot(manifest, home_path) == before,
             "a plain update to the older release was not refused unchanged")
        rollback = command("undo-rollback", ["update", "--rollback-to", head("old"), "--yes"])
        need(cli_success(processes, "undo-rollback", rollback) and "Rollback plan" in processes.text("undo-rollback"),
             "explicit Undo failed; command exit " + str(rollback["exit"]) + cli_ci_stderr_detail(processes, manifest, "undo-rollback", "Undo"))
        version = command("undo-version", ["--version"])
        need(version["exit"] == 0 and processes.text("undo-version") == "elastos " + manifest["old"]["version"] + "\n",
             "Undo did not restore the previous Runtime")
        expect("old")
        need(cli_home_snapshot(manifest, home_path) == before, "Undo changed user data or installed another release")
        cli_verify_setup_support(root, cli_support_view(manifest, "old"), home_path)
        readiness = restart("undo", "undo-home-start", "old")
        need(browser.wait(timeout=420) == 0 and (stages / "undone.json").exists(),
             "Home did not reconnect the same account and preference on the previous release"
             + cli_ci_stderr_detail(processes, manifest, "home-system", "System"))
        need(cli_home_snapshot(manifest, home_path) == before, "the previous release changed data written after the update")
        return {"status": "passed", "version": manifest["old"]["version"], "head_cid": head("old"),
                "refused_plain_update": {"boundary": "older than installed release", **refused}, "rollback": rollback,
                "readiness": readiness,
                "user_data": {"before_update": sorted(relative for group in before["preserved"].values() for relative in group),
                              "after_update": ["state/written-after-update"]},
                "account": {"written": cli_json(stages / "post-update-item.json"), "after_undo": cli_json(stages / "undone.json")}}

    try:
        proof = start("initial-home-start", "old")
        need(cli_home_snapshot(manifest, home_path) == before, "initial Home changed installed trust, identity or preserved data")
        if manifest["proof_kind"] == "real-runtime":
            proof["system"], browser = cli_home_system_journey(processes, manifest, home_path, publish, update_with_cli)
            identities.append(proof["system"]["host"])
            # The System update replaced the installed release; shutdown must keep exactly that.
            expect(CLI_SYSTEM_PHASE)
            cli_verify_setup_support(root, cli_support_view(manifest, "new"), home_path)
            proof["undo"] = undo(browser)
    except Exception as error:
        failure = error
        # Before the stop below rewrites controller status: the failed start as it was.
        evidence["diagnostics"] = cli_failure_diagnostics(processes, manifest, [home_path], "at-failure")
    finally:
        try:
            if process is not None:
                exit_code = stop()
        except Exception as error:
            cleanup_failure = error
    evidence["controller_exit"] = exit_code
    if hasattr(processes, "output"):
        try:
            for stream in ("stdout", "stderr"):
                evidence["controller_" + stream + "_sha256"] = digest(processes.output / ("initial-home-start." + stream))
        except OSError:
            if cleanup_failure is None:
                cleanup_failure = ValueError("initial Home log evidence is unavailable")
    if failure is not None or cleanup_failure is not None or exit_code != 0:
        detail = str(failure) if failure is not None else "installed Home controller did not stop cleanly"
        detail += "; controller exit " + str(exit_code)
        if cleanup_failure is not None:
            detail += "; cleanup: " + str(cleanup_failure)
        detail += cli_ci_stderr_detail(processes, manifest, "initial-home-start", "controller")
        raise ValueError(detail) from failure
    need(proof is not None, "initial Home readiness proof is absent")
    released()
    need(cli_home_snapshot(manifest, home_path) == before, "Home shutdown changed installed trust, identity or preserved data")
    proof["desktop_opener"] = {"suppressed": True, "sha256": binding["sha256"], "manual_ux": "requires operator acceptance"}
    proof.update({key: value for key, value in evidence.items() if key.startswith("controller_")})
    proof["cleanup"] = {"controller_exit": process.returncode, "reaped": True, "groups_absent": True,
                        "ports_released": True, "locks_released": True, "coordinates_removed": True, "data_preserved": True}
    return proof


def cli_home_system_journey(processes, manifest, home_path, publish, update_with_cli):
    """Through the real pages, the supported update path from the published release:
    create the first account and see Home up to date; the published release's System refuses
    a release whose support changed (its known limit) and keeps Home; its CLI updates to
    this source; this source's System refuses a tampered offer, applies the next release and
    reconnects; the account stores a preference and the browser stays open for Undo."""
    need(publish is not None, "installed System journey requires the fixture publisher")
    node = shutil.which("node")
    need(node is not None and "PLAYWRIGHT_BROWSERS_PATH" in os.environ, "installed System journey requires Node and Playwright")
    output = processes.output / "home-system"
    output.mkdir(mode=0o700)
    env = cli_environment(home_path)
    env.update(PLAYWRIGHT_BROWSERS_PATH=os.environ["PLAYWRIGHT_BROWSERS_PATH"],
               HOME_VIRTUAL_AUTH_PROFILE=str(output / "browser-profile"), HOME_VIRTUAL_AUTH_CLEANUP="0",
               HOME_VIRTUAL_AUTH_SHELL_SWITCH="0", HOME_VIRTUAL_AUTH_UPDATE_DIR=str(output),
               HOME_VIRTUAL_AUTH_UPDATE_VERSIONS=" ".join(manifest[name]["version"] for name in ("old", "new", CLI_SYSTEM_PHASE)))
    browser = processes.spawn([node, str(Path(__file__).with_name("home-passkey-virtual-auth-smoke.mjs"))],
                              env, home_path, "home-system")
    binary = home_path / ".local/bin/elastos"
    status_path = home_path / CLI_DATA / "update-controller/status.json"
    coords_path = home_path / CLI_DATA / "gateway-runtime-coords.json"
    source = lambda: source_for_home(home_path)["installed_version"]  # noqa: E731
    shas = {name: manifest["files"][manifest["publications"][name]["binary"]]["sha256"] for name in ("old", "new", CLI_SYSTEM_PHASE)}
    # Updates replace the installed executable while its processes are owned.
    for path in (binary, home_path / CLI_DATA / "update-controller/runtime"):
        processes.roots[str(path)] = set(shas.values())

    def reached(stage, seconds, message):
        deadline = time.monotonic() + seconds
        while browser.poll() is None and not (output / (stage + ".json")).exists() and time.monotonic() < deadline:
            time.sleep(.2)
        need((output / (stage + ".json")).exists() and browser.poll() is None,
             message + cli_ci_stderr_detail(processes, manifest, "home-system", "System"))

    def installed(release):
        return digest(binary) == shas[release] and source() == manifest[release]["version"]

    reached("up-to-date", 240, "installed Home sign-up or up-to-date System state failed")
    warnings_before = cli_provider_warnings(home_path)
    # The published release's System applies only releases with unchanged support (#246):
    # it refuses this one, restores and keeps Home on its own release.
    publish("new")
    reached("frozen-refused", 330, "the published release's System did not refuse the changed-support release plainly")
    frozen = cli_private_json(status_path)
    frozen_host = cli_process_identity(frozen["host_pid"]) if frozen.get("host_pid") else None
    need(frozen["phase"] == "restored" and installed("old") and frozen_host is not None
         and cli_process_executable(frozen_host["pid"]) == str(binary),
         "the published release's System did not keep Home on its release after refusing")
    cli_update = update_with_cli(frozen_host)
    reached("updated", 420, "Home did not reconnect the same account on the new release after the CLI update")
    # alpha.6: the updated Runtime refused the installed protected-content providers
    # while System said "up to date". check-result requires `new` to be empty.
    warnings_after = cli_provider_warnings(home_path)
    # This source's System refuses a tampered offer: the controller records the failure
    # while staging and the same Home process and generation keep running.
    home_before = {key: cli_private_json(coords_path)[key] for key in ("pid", "generation")}
    publish("tampered-binary")
    reached("refused", 330, "System did not refuse the tampered fixture plainly")
    refusal = cli_private_json(status_path)
    need(refusal["phase"] == "failed", "the controller did not record the tampered release as refused")
    need({key: cli_private_json(coords_path)[key] for key in ("pid", "generation")} == home_before,
         "a refused release restarted Home")
    need(installed("new"), "refused update changed the installed release")
    publish(CLI_SYSTEM_PHASE)
    reached("post-update-item", 420, "System did not update to the next release, reconnect and store the account preference")
    status = cli_private_json(status_path)
    host = cli_process_identity(status["host_pid"]) if status.get("host_pid") else None
    need(status["phase"] == "updated" and status["current_version"] == manifest[CLI_SYSTEM_PHASE]["version"]
         and installed(CLI_SYSTEM_PHASE) and host is not None
         and cli_process_executable(host["pid"]) == str(binary), "controller did not restart Home on the next release")
    stages = ("up-to-date", "frozen-offer", "frozen-refused", "updated", "tampered-offer", "refused",
              "next-offer", "next-updated", "post-update-item")
    return {"stages": {name: cli_json(output / (name + ".json")) for name in stages}, "host": host,
            "frozen_support": {"phase": frozen["phase"], "version": manifest["old"]["version"], "kept_home": True},
            "cli_update": cli_update,
            "provider_warnings": {"before": warnings_before, "after": warnings_after,
                                  "new": sorted(set(warnings_after) - set(warnings_before))}}, browser


def cli_provider_warnings(home_path):
    """Provider warnings and errors of the current Home start (the controller truncates the log per start)."""
    text = (home_path / CLI_DATA / "update-controller/runtime.log").read_text(errors="replace")
    lines = (re.sub(r"\x1b\[[0-9;]*m", "", line) for line in text.splitlines())
    return sorted({match[1].strip() for line in lines
                   if "provider" in line and (match := re.search(r"\b(?:WARN|ERROR)\b(.*)", line))})


def cli_run(config, output, local_rehearsal=None):
    manifest = cli_admit(config, local_rehearsal)
    root = Path(config["root"])
    need(output == cli_path(root, "results"), "CLI run requires its fresh root/results directory")
    output.mkdir(mode=0o700, exist_ok=False)
    processes = CliProcesses(output)
    result = {"schema": "elastos.update-hop.result/v1", "mode": CLI_MODE,
              "proof_kind": manifest["proof_kind"], "approval": manifest["approval"],
              "proof_scope": manifest.get("proof_scope", "production-positive"),
              "signer_did": manifest["signer_did"], "channel": manifest["channel"],
              "source": manifest["source"], "old": manifest["old"], "new": manifest["new"],
              "manifest_sha256": config["immutable"]["sha256"], "reference": manifest["reference"],
              "retrieval_reference": os.environ.get("ELASTOS_CI_FIXTURE_REFERENCE", ""),
              "started_at": datetime.datetime.now(datetime.timezone.utc).isoformat(), "paths": {}, "coordination": {}, "setup": {}}
    if manifest.get("build"):
        result["build"] = cli_json(cli_path(root, manifest["build"]))
    if "previous" in manifest:
        pin = cli_previous_pin()
        result["previous_release"] = {"version": pin["version"], "release_cid": pin["release_cid"], "release_sha256": pin["release_sha256"]}
    holder = output / "homes/holder"
    holder.mkdir(mode=0o700, parents=True)
    holder_data = holder / CLI_DATA
    holder_bin = holder / ".local/bin/elastos"
    # The old release installs with its own installer; this source's installer installs new.
    installers = {"old": cli_path(root, manifest.get("previous", manifest)["installer"]), "new": cli_path(root, manifest["installer"])}
    host, bootstrap, port, installer_bootstrap, carrier_port, published = None, None, None, None, None, None
    holder_config = holder_data / "config.toml"
    holder_labels, holder_config_text = [], None

    def command(home_path, args, label, evidence=None):
        reply = processes.command([str(home_path / ".local/bin/elastos"), *args],
                                  cli_environment(home_path), home_path, label)
        if evidence is not None:
            evidence.update(reply)
        if home_path != holder:
            result["coordination"][label] = cli_coordination(home_path)
        return reply

    def track(home_path, mapping):
        # Support executables change with the release, so each path admits every release's bytes.
        for relative, source_relative in mapping.items():
            if relative == ".local/bin/elastos" or relative.startswith(CLI_DATA + "/bin/"):
                binding = manifest["files"][source_relative]
                if binding["mode"] & 0o111:
                    processes.roots.setdefault(str(cli_path(home_path, relative)), set()).add(binding["sha256"])

    def phase(name):
        nonlocal host, bootstrap, port, published
        if name == published:
            return
        cli_inventory(root, manifest)
        need(digest(root / "manifest.json") == config["immutable"]["sha256"], "manifest changed during run")
        if host is not None:
            need(host.poll() is None, "holder exited before phase switch")
            os.killpg(host.pid, signal.SIGTERM)
            host.wait(timeout=10)
            need(lock_state(holder_data / "host-process.lock") != "held", "holder lock survives phase stop")
        publication = manifest["publications"][name]
        snapshot = {CLI_PUBLISHER + "/release-head.json": publication["head"],
                    CLI_PUBLISHER + "/release.json": publication["release"],
                    CLI_PUBLISHER + "/publish-state.json": publication["receipt"],
                    CLI_PUBLISHER + "/artifacts/elastos-" + manifest["platform"]: publication["binary"],
                    CLI_PUBLISHER + "/artifacts/components-" + manifest["platform"] + ".json": publication["components"],
                    CLI_PUBLISHER + "/artifacts/model-catalog.json": publication["catalogue"]}
        support = cli_support_view(manifest, "old" if name == "old" else "new")
        snapshot.update({CLI_PUBLISHER + "/artifacts/" + artifact: relative
                         for artifact, relative in support.get("setup", {}).get("artifacts", {}).items()})
        cli_copy(root, manifest, snapshot, holder_data)
        label = "holder-" + str(len(processes.processes))
        holder_labels.append(label)
        host = processes.spawn([str(holder_bin), "gateway", "--addr", "127.0.0.1:" + str(port),
                                "--cache-dir", str(holder / "gateway-cache")], cli_environment(holder), holder, label)
        deadline = time.monotonic() + 40
        while host.poll() is None and time.monotonic() < deadline:
            if health(port) and lock_state(holder_data / "host-process.lock") == "held":
                break
            time.sleep(0.2)
        need(host.poll() is None and health(port), "holder failed phase readiness")
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        with opener.open(f"http://127.0.0.1:{port}/.well-known/elastos/carrier-bootstrap.json?role=publisher", timeout=5) as response:
            current = json.load(response)
        cli_holder_bootstrap(current, holder_node_id)
        if bootstrap is not None:
            need(current["node_id"] == bootstrap["node_id"], "holder node changed between phases")
        bootstrap = current
        installer_bootstrap.publication, published = publication, name

    def install(home_path, label, release):
        env = cli_environment(home_path)
        env.update(ELASTOS_PUBLISHER_GATEWAY=installer_bootstrap.url,
                   ELASTOS_SOURCE_CONNECT_TICKET=bootstrap["ticket"], ELASTOS_PUBLISHER_NODE_ID=bootstrap["node_id"])
        installer_bootstrap.enabled = True
        try:
            reply = processes.command(["/bin/bash", str(installers[release]), "--install-only"], env, home_path, label, timeout=180)
            result["coordination"][label] = cli_coordination(home_path, required=reply["exit"] == 0)
            return reply
        finally:
            installer_bootstrap.enabled = False

    def setup(home_path, label, release):
        view = cli_support_view(manifest, release)
        track(home_path, {CLI_DATA + "/" + target: relative for target, relative in cli_setup_files(view).items()})
        evidence = {"status": "failed", "release": release}
        result["setup"][label] = evidence
        reply = command(home_path, ["setup"], label, evidence)
        need(cli_success(processes, label, reply), "installed setup failed; command exit " + str(reply["exit"])
             + cli_ci_stderr_detail(processes, manifest, label, label))
        for filename, key in (("components.json", "components"), ("model-catalog.json", "catalogue")):
            need(digest(home_path / CLI_DATA / filename) == manifest["files"][manifest["publications"][release][key]]["sha256"],
                 "installed setup " + key + " hash differs")
        cli_verify_setup_support(root, view, home_path)
        evidence["status"] = "passed"

    def prepare(home_path, label, release):
        view = cli_support_view(manifest, release)
        cli_verify_setup_support(root, view, home_path)
        setup_targets = cli_setup_files(view) if "setup" in view else {}
        mapping = {target: relative for target, relative in view["consumer"]["files"].items()
                   if target not in setup_targets}
        cli_copy(root, manifest, mapping, home_path / CLI_DATA)
        track(home_path, {CLI_DATA + "/" + target: relative for target, relative in view["consumer"]["files"].items()})
        need(command(home_path, ["node", "info", "--json"], label + "-identity")["exit"] == 0, "consumer fixture identity failed")
        for group, values in cli_state(manifest, home_path)["preserved"].items():
            need(all(value for value in values.values()), group + " preservation fixture absent")

    def verify(home_path, publication_name, label, fresh):
        """fresh: written by an installer, which caches no head; else by an update."""
        observation = cli_state(manifest, home_path)
        publication = manifest["publications"][publication_name]
        for key in ("binary", "components", "catalogue"):
            need(observation[key] == manifest["files"][publication[key]]["sha256"], "installed " + key + " hash differs")
        version = manifest[publication_name]["version"]
        reply = command(home_path, ["--version"], label)
        need(reply["exit"] == 0 and processes.text(label) == "elastos " + version + "\n"
             and not processes.text(label, "stderr"), "installed exact version differs")
        stored = observation["sources"]["sources"][0]
        need(stored["publisher_dids"] == [manifest["signer_did"]] and stored["channel"] == manifest["channel"]
             and stored["publisher_node_id"] == bootstrap["node_id"] and stored["connect_ticket"]
             and stored["gateways"] == [installer_bootstrap.url]
             and stored["install_path"] == str(home_path / ".local/bin/elastos")
             and stored["installed_version"] == version, "installed trust/config binding differs")
        expected = cli_expected_installed_metadata(manifest, publication_name)
        if fresh:
            need(not stored["head_cid"], "frozen installer cached a head CID")
            need(observation["installed_metadata"] in (None, expected), "installed metadata differs")
            if observation["installed_metadata"] is None:
                need(cli_legacy_installed_metadata(home_path) == expected, "legacy installed metadata hash differs")
        else:
            need(observation["installed_metadata"] == expected, "installed consumed metadata hash differs")
            need(stored["head_cid"] == manifest["files"][publication["head"]]["cid"],
                 "installed consumed head CID differs")
        return observation

    try:
        holder_mapping = manifest["holder"]["files"]
        binaries = {target: relative for target, relative in holder_mapping.items() if "/ipfs-repo/" not in target}
        cli_copy(root, manifest, binaries, holder)
        need(not holder_config.exists(), "holder fixture must leave transport configuration to the observer")
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as probe:
            probe.bind(("127.0.0.1", 0))
            carrier_port = probe.getsockname()[1]
        holder_config_text = ('carrier_bind_addr = "127.0.0.1:' + str(carrier_port) + '"\n'
                              'gateway_public_publisher_bootstrap = true\n')
        holder_config.write_text(holder_config_text)
        holder_config.chmod(0o600)
        track(holder, manifest["holder"]["files"])
        kubo_env = cli_environment(holder)
        kubo_env["IPFS_PATH"] = str(holder_data / "ipfs-repo")
        need(processes.command([str(holder_data / "bin/kubo"), "init", "--profile=test"], kubo_env, holder, "holder-kubo-init")["exit"] == 0,
             "disposable Kubo fixture initialization failed")
        cli_copy(root, manifest, {target: relative for target, relative in holder_mapping.items() if target not in binaries}, holder)
        identity = command(holder, ["node", "info", "--json"], "holder-identity")
        need(identity["exit"] == 0, "disposable holder identity failed")
        holder_did = cli_json(output / "holder-identity.stdout")["did"]
        holder_node_id = cli_holder_node_id(holder_did)
        need(holder_did != manifest["signer_did"], "holder and signer identities coincide")
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", 0))
            port = probe.getsockname()[1]
        installer_bootstrap = CliBootstrap(root, manifest)
        phase("old")
        consumer = output / "homes/cli"
        consumer.mkdir(mode=0o700)
        main_bin = str(consumer / ".local/bin/elastos")
        processes.roots[main_bin] = {manifest["files"][publication["binary"]]["sha256"] for publication in manifest["publications"].values()}
        result["paths"]["m1-install"] = {"status": "failed", "checks": {}}
        reply = install(consumer, "m1-install", "old")
        need(cli_success(processes, "m1-install", reply), "fresh installer failed")
        setup(consumer, "m1-setup", "old")
        verify(consumer, "old", "m1-version", fresh=True)
        result["paths"]["m1-install"]["checks"]["positive"] = {"status": "passed", **reply}
        prepare(consumer, "main", "old")
        before = cli_state(manifest, consumer)
        if "initial_home" in manifest:
            initial = output / "homes/initial-home"
            initial.mkdir(mode=0o700)
            processes.roots[str(initial / ".local/bin/elastos")] = {manifest["files"][manifest["publications"]["old"]["binary"]]["sha256"]}
            need(cli_success(processes, "initial-home-install", install(initial, "initial-home-install", "old")), "initial Home installer failed")
            setup(initial, "initial-home-setup", "old")
            verify(initial, "old", "initial-home-version", fresh=True)
            prepare(initial, "initial-home", "old")
            # The updates refresh support while Home runs its providers; cleanup admits both releases' bytes.
            track(initial, {CLI_DATA + "/" + target: relative for target, relative in cli_setup_files(cli_support_view(manifest, "new")).items()})
            result["paths"]["m1-install"]["checks"]["initial-home"] = {
                "status": "failed", "proof_scope": "installed-initial-home", "support_scope": "frozen-source-home"}
            result["paths"]["m1-install"]["checks"]["initial-home"] = cli_initial_home(
                processes, manifest, initial, result["paths"]["m1-install"]["checks"]["initial-home"], phase, root)
        result["paths"]["m2-discovery"] = {"status": "failed", "checks": {}}
        if result["proof_scope"] in ("production-positive", "ci-rehearsal", "local-rehearsal"):
            phase("new")
            result["paths"]["m2-discovery"] = {"status": "failed", "checks": {}}
            reply = command(consumer, ["update", "--check"], "m2-check")
            need(cli_success(processes, "m2-check", reply) and "Discovery: Carrier" in processes.text("m2-check")
                 and cli_state(manifest, consumer) == before, "plain Carrier check failed or changed files")
            result["paths"]["m2-discovery"]["checks"]["check"] = {"status": "passed", **reply}
            apply = {"status": "failed"}
            result["paths"]["m2-discovery"]["checks"]["apply"] = apply
            reply = command(consumer, ["update"], "m2-apply", apply)
            if not (cli_success(processes, "m2-apply", reply) and "Discovery: Carrier" in processes.text("m2-apply")):
                raise ValueError("plain Carrier apply failed; command exit " + str(reply["exit"])
                                 + cli_ci_stderr_detail(processes, manifest, "m2-apply", "m2-apply"))
            need(result["coordination"]["m2-apply"]["pid"] == reply["pid"], "apply coordination PID differs from its owned command")
            after = verify(consumer, "new", "m2-version", fresh=False)
            need(after["coordination"] == result["coordination"]["m2-apply"], "version command changed Runtime coordination metadata")
            cli_verify_setup_support(root, cli_support_view(manifest, "new"), consumer)
            expected_sources = json.loads(json.dumps(before["sources"]))
            expected_sources["sources"][0].update(installed_version=manifest["new"]["version"], head_cid=manifest["files"][manifest["publications"]["new"]["head"]]["cid"])
            need(after["sources"] == expected_sources and after["preserved"] == before["preserved"] and after["data"] == before["data"], "config/data/support preservation differs")
            apply["status"] = "passed"
            reply = command(consumer, ["update"], "m2-repeat")
            need(cli_success(processes, "m2-repeat", reply) and "Installed release is up to date." in processes.text("m2-repeat")
                 and cli_state(manifest, consumer) == after, "repeat update changed the installed fixture")
            result["paths"]["m2-discovery"]["checks"]["repeat"] = {"status": "passed", **reply}
        for case in manifest["selectors"]["m1-install"]["refusals"]:
            phase(case)
            fresh = output / ("homes/m1-" + case)
            fresh.mkdir(mode=0o700)
            label = "m1-" + case
            reply = install(fresh, label, "new")
            unchanged = not files(fresh / ".local") and not files(fresh / CLI_DATA)
            refusal = cli_refusal(case, reply, processes.text(label), processes.text(label, "stderr"), unchanged)
            result["paths"]["m1-install"]["checks"][case] = refusal
            need(refusal["status"] == "passed", "fresh installer refusal boundary differs")
            # Refusals are offered to this source's Runtime: the seed installs new, and every
            # refusal publication is labelled with the release after it.
            phase("new")
            target = output / ("homes/m2-" + case)
            target.mkdir(mode=0o700)
            processes.roots[str(target / ".local/bin/elastos")] = {manifest["files"][manifest["publications"]["new"]["binary"]]["sha256"]}
            need(cli_success(processes, "seed-" + case, install(target, "seed-" + case, "new")), "refusal consumer installation failed")
            setup(target, "seed-setup-" + case, "new")
            verify(target, "new", "seed-version-" + case, fresh=True)
            prepare(target, "seed-" + case, "new")
            original = cli_state(manifest, target)
            phase(case)
            if case.startswith("wrong-signer-"):
                check_label = "m2-check-" + case
                check_reply = command(target, ["update", "--check"], check_label)
                check_refusal = cli_refusal(case, check_reply, processes.text(check_label), processes.text(check_label, "stderr"), cli_state(manifest, target) == original)
                result["paths"]["m2-discovery"]["checks"][case + "-check"] = check_refusal
                need(check_refusal["status"] == "passed", "Carrier check refusal boundary differs")
            label = "m2-" + case
            reply = command(target, ["update"], label)
            refusal = cli_refusal(case, reply, processes.text(label), processes.text(label, "stderr"), cli_state(manifest, target) == original)
            result["paths"]["m2-discovery"]["checks"][case] = refusal
            need(refusal["status"] == "passed", "plain update refusal boundary differs")
        for entry in result["paths"].values():
            entry["status"] = "passed"
    except (OSError, ValueError, TypeError, KeyError, StopIteration, subprocess.SubprocessError, KeyboardInterrupt) as error:
        result["failure"] = cli_safe_error(error)
        homes = sorted(path for path in (output / "homes").iterdir() if path.is_dir()) if (output / "homes").is_dir() else []
        result["diagnostics"] = cli_failure_diagnostics(processes, manifest, homes, "after-failure")
    finally:
        result["cleanup"] = processes.cleanup()
        try:
            if installer_bootstrap is not None:
                installer_bootstrap.close()
                result["transport"] = {"m2_http_fallback_requests": installer_bootstrap.fallback_requests,
                                       "installer_bootstrap_closed": True, "bootstrap_errors": installer_bootstrap.errors}
                need(installer_bootstrap.fallback_requests == 0, "update attempted an HTTP fallback")
                need(installer_bootstrap.errors == 0, "installer bootstrap request failed")
            result["holder_output"] = [{"label": label, "clean": cli_clean_output(processes, label),
                                        "stdout_sha256": digest(output / (label + ".stdout")),
                                        "stderr_sha256": digest(output / (label + ".stderr"))} for label in holder_labels]
            need(all(entry["clean"] for entry in result["holder_output"]), "holder output contains an endpoint error")
            cli_inventory(root, manifest)
            need(digest(root / "manifest.json") == config["immutable"]["sha256"], "manifest changed during run")
            if local_rehearsal is not None:
                need(cli_local_rehearsal(local_rehearsal) == manifest["source"], "local hop source changed during run")
            need(lock_state(holder_data / "host-process.lock") != "held" and (port is None or not health(port)), "holder remains active after cleanup")
            if carrier_port is not None:
                need(holder_config.read_text() == holder_config_text and holder_config.stat().st_mode & 0o777 == 0o600,
                     "holder transport configuration changed")
        except (OSError, ValueError, KeyError) as error:
            result["cleanup"]["errors"].append(cli_safe_error(error))
        result["cleanup"]["passed"] = not result["cleanup"]["errors"]
        result["status"] = "passed" if "failure" not in result and result["cleanup"]["passed"] and len(result["paths"]) == 2 and all(entry["status"] == "passed" for entry in result["paths"].values()) else "failed"
        write(output / "result.json", result)
    print(str(output / "result.json"))
    return 0 if result["status"] == "passed" else 1


def cli_check_result(result):
    """The CI verdict on a run's result.json: the journey passed along the supported path
    from the pinned published release (its System refused changed support, its CLI updated),
    the update added no provider warnings, and the hop was undone explicitly."""
    need(result.get("schema") == "elastos.update-hop.result/v1" and result.get("status") == "passed",
         "update hop failed: " + str(result.get("failure", "status " + str(result.get("status")))))
    home = result["paths"]["m1-install"]["checks"]["initial-home"]
    new = home["system"]["provider_warnings"]["new"]
    need(not new, "the update added provider warnings: " + "; ".join(new))
    previous = result.get("previous_release")
    need(previous is not None and result["old"]["version"] == previous["version"],
         "the hop did not start from the pinned published release")
    need(home["system"].get("frozen_support", {}).get("phase") == "restored"
         and home["system"].get("cli_update", {}).get("status") == "passed",
         "the published release did not refuse changed support in System and update with its CLI")
    undo = home.get("undo", {})
    need(undo.get("status") == "passed" and undo["version"] == previous["version"]
         and undo["refused_plain_update"]["exit"] != 0 and undo["rollback"]["exit"] == 0,
         "the explicit Undo to the published release did not pass")
    return {"status": "passed", "from": previous["version"], "to": result["new"]["version"],
            "provider_warnings_new": [], "undo": {"version": undo["version"], "plain_update_refused": True}}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("operation", choices=("inspect", "run", "compare", "prepare-ci-disk", "pin-previous-release", "fetch-previous-release",
                                              "build-ci-hop", "generate-ci-hop", "check-result"))
    parser.add_argument("input", type=Path, nargs="?")
    parser.add_argument("output", type=Path, nargs="?")
    parser.add_argument("--root", type=Path, help="physical downloaded CLI fixture root")
    parser.add_argument("--runtime", type=Path, help="built Runtime for CI refusal generation")
    parser.add_argument("--next-runtime", type=Path, help="compiled Runtime for the release after the published one")
    parser.add_argument("--system-runtime", type=Path, help="compiled Runtime for the release after that, which System applies")
    parser.add_argument("--build-receipt", type=Path, help="exact CI Runtime build receipt")
    parser.add_argument("--support-home", type=Path, help="built source-home data directory for CI refusal generation")
    parser.add_argument("--previous", type=Path, help="fetch-previous-release output: the pinned published release")
    parser.add_argument("--localhost-metadata", type=Path, help="original qualified localhost capsule metadata archive")
    parser.add_argument("--local-rehearsal", metavar="REFERENCE", help="approved local-rehearsal:<source-commit>:<fixture-name> selector")
    args = parser.parse_args()
    os.umask(0o077)
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt("terminated")))
    try:
        need(args.localhost_metadata is None or args.operation == "generate-ci-hop",
             "localhost metadata input applies only to fixture generation")
        if args.local_rehearsal is not None:
            need(args.operation in ("build-ci-hop", "generate-ci-hop", "inspect", "run"),
                 "local rehearsal applies only to CLI fixture build, generation, inspect and run")
        if args.operation in ("prepare-ci-disk", "pin-previous-release"):
            need(all(value is None for value in (args.input, args.output, args.root, args.runtime, args.next_runtime, args.system_runtime, args.build_receipt, args.support_home, args.previous)), "this operation has fixed inputs")
            print(json.dumps(cli_prepare_ci_disk() if args.operation == "prepare-ci-disk" else cli_pin_previous_release()))
            return 0
        need(args.input is not None, "fixture input required")
        if args.operation == "fetch-previous-release":
            need(args.output is not None and args.root is None, "previous release fetch requires a cache and a fresh output")
            print(json.dumps(cli_fetch_previous_release(args.input, args.output)))
            return 0
        if args.operation == "check-result":
            print(json.dumps(cli_check_result(cli_json(args.input))))
            return 0
        if args.operation == "build-ci-hop":
            need(args.runtime is not None and args.previous is not None and args.output is None and args.root is None,
                 "hop build requires a Runtime and the previous release")
            print(json.dumps(cli_build_hop(args.input, args.runtime, args.previous, args.local_rehearsal)))
            return 0
        if args.operation == "generate-ci-hop":
            need(all(path is not None for path in (args.runtime, args.next_runtime, args.system_runtime, args.build_receipt, args.support_home, args.previous))
                 and args.output is None and args.root is None,
                 "hop generation requires the three Runtimes, build receipt, support-home and previous release inputs")
            print(json.dumps(cli_generate_hop(args.input, args.runtime, args.next_runtime, args.system_runtime, args.build_receipt, args.support_home,
                                             args.previous, args.local_rehearsal, args.localhost_metadata)))
            return 0
        value = read(args.input)
        if args.local_rehearsal is not None:
            need(value.get("mode") == CLI_MODE, "local rehearsal applies only to CLI fixtures")
        if args.root is not None:
            need(value.get("mode") == CLI_MODE, "--root applies only to CLI fixtures")
            value["root"] = str(args.root)
        if args.operation == "run":
            need(args.output is not None and args.output.is_absolute(), "run requires a new absolute receipt directory")
            return cli_run(value, args.output, args.local_rehearsal) if args.local_rehearsal is not None else run(value, args.output)
        if args.local_rehearsal is not None:
            result = cli_inspect(value, args.local_rehearsal)
        else:
            result = inspect(value) if args.operation == "inspect" else compare(value)
        print(json.dumps(result, indent=2))
        return 0
    except (OSError, ValueError, TypeError, KeyError, StopIteration, subprocess.SubprocessError) as error:
        reason = cli_safe_error(error) if isinstance(locals().get("value"), dict) and value.get("mode") == CLI_MODE else str(error)
        result = {"status": "unavailable", "reason": reason}
        if hasattr(error, "capacity"):
            result["capacity"] = error.capacity
        print(json.dumps(result), file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
