#!/usr/bin/env python3
"""Issue #89: prove gateway-role Kubo stays up through real idle time in CI.

The candidate Runtime owns role selection. The accepted frozen Runtime is a
Carrier-only consumer. The separate workflow fixture proves user idle recovery.
This helper qualifies isolated CI artifacts; it does not install a seed package.
"""

import base64
import hashlib
import importlib.util
import json
import os
import re
import shutil
import socket
import subprocess
import time
import urllib.parse
import urllib.request
from pathlib import Path


def sha(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def identity(pid):
    root = Path("/proc") / str(pid)
    return {"pid": pid, "start_time": (root / "stat").read_text().rsplit(")", 1)[1].split()[19],
            "exe": os.readlink(root / "exe")}


def port(kind=socket.SOCK_STREAM):
    with socket.socket(socket.AF_INET, kind) as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def main():
    assert os.environ["GITHUB_ACTIONS"] == "true" and os.environ["RUNNER_OS"] == "Linux"
    workspace = Path(os.environ["GITHUB_WORKSPACE"])
    accepted, output = Path(os.environ["ACCEPTED_INPUT"]), Path(os.environ["HOLDER_OUTPUT"])
    observer_path = workspace / "observer/scripts/update-hop-compare.py"
    assert sha(observer_path) == os.environ["OBSERVER_SHA256"]
    spec = importlib.util.spec_from_file_location("accepted_observer", observer_path)
    observer = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(observer)
    processes = observer.CliProcesses(output)
    root = Path.home() / ".local/share/elastos-issue89-always-on"
    assert not root.exists() and not root.is_symlink()
    root.mkdir(mode=0o700, parents=True)
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    result = {"schema": "elastos.issue89.holder-always-on/v1", "passed": False,
              "clock": "real monotonic elapsed idle; no coordination clock changes",
              "source_commit": os.environ["PROVIDER_COMMIT"], "source_tree": os.environ["PROVIDER_TREE"],
              "consumer_source_commit": os.environ["SOURCE_COMMIT"],
              "consumer_source_tree": os.environ["SOURCE_TREE"], "seed_package": False,
              "service_restarts": 0, "provider_restarts": 0, "source_modified": False}

    def environment(home):
        return {**observer.cli_environment(home), "XDG_DATA_HOME": str(home / ".local/share"),
                "ELASTOS_DATA_DIR": str(home / ".local/share/elastos")}

    def command(home, args, label):
        reply = processes.command(args, environment(home), home, "always-on-" + label, timeout=90)
        assert reply["exit"] == 0, label + " failed; see fixture logs"
        return processes.text("always-on-" + label).strip()

    def get(url, bound=65536, timeout=10):
        with opener.open(url, timeout=timeout) as response:
            assert response.status == 200
            payload = response.read(bound + 1)
            assert len(payload) <= bound
            return payload

    def post(url, request, token=None, bound=65536, timeout=90):
        headers = {"Content-Type": "application/json"}
        if token is not None:
            headers["Authorization"] = "Bearer " + token
        request = urllib.request.Request(url, data=json.dumps(request).encode(), headers=headers, method="POST")
        return json.loads(get(request, bound, timeout))

    def ready(owner, url):
        deadline = time.monotonic() + 60
        while owner.poll() is None and time.monotonic() < deadline:
            try:
                get(url, timeout=2)
                return
            except (OSError, urllib.error.URLError):
                time.sleep(.2)
        raise AssertionError("Runtime readiness failed")

    def attach(data, owner, name, kind):
        path = data / name
        assert not path.is_symlink() and path.stat().st_mode & 0o077 == 0
        coords = json.loads(path.read_bytes())
        assert coords["pid"] == owner.pid and coords["runtime_kind"] == kind
        parsed = urllib.parse.urlsplit(coords["api_url"])
        assert parsed.scheme == "http" and parsed.hostname == "127.0.0.1" and parsed.port
        assert parsed.path in ("", "/") and parsed.username is None and parsed.password is None
        assert not parsed.query and not parsed.fragment
        api = coords["api_url"].rstrip("/")
        reply = post(api + "/api/auth/attach", {"secret": coords["attach_secret"], "scope": "shell"})
        assert reply["session_type"] == "shell" and isinstance(reply["token"], str) and reply["token"]
        return api, reply["token"]

    def carrier_fetch(auth, cid, expected):
        bound = 4 * ((expected.stat().st_size + 2) // 3) + 65536
        response = post(auth[0] + "/api/provider/content/fetch", {"cid": cid}, auth[1], bound)
        assert response["status"] == "ok" and response["data"]["cid"] == cid
        availability = response["data"]["availability"]
        assert availability["provider"] == "carrier-availability" and availability["policy"] == "carrier_provider_invoke"
        assert availability["transport"] == "carrier-provider-plane" and availability["status"] == "network_available"
        payload = base64.b64decode(response["data"]["data"], validate=True)
        assert len(payload) == expected.stat().st_size and hashlib.sha256(payload).hexdigest() == sha(expected)
        return payload

    try:
        template = json.loads((workspace / "source/components.json").read_bytes())
        builds = {"runtime": json.loads((output / "gateway-runtime-build.json").read_bytes()),
                  "provider": json.loads((output / "provider-build.json").read_bytes()),
                  "localhost": json.loads((output / "localhost-fixture-build.json").read_bytes())}
        accepted_build = json.loads((output / "accepted-build.json").read_bytes())
        accepted_runtime = next(item for item in accepted_build["binaries"] if item["name"] == "elastos")
        assert accepted_runtime["size"] == 114187264
        assert sha(accepted / "elastos") == accepted_runtime["sha256"]
        assert not builds["runtime"]["seed_package"] and not builds["localhost"]["seed_package"]
        helper_root = Path(builds["runtime"]["helper_root"])
        assert str(helper_root) == "/home/runner/.local/share/elastos-issue89-candidate/source"
        assert builds["runtime"]["source_commit"] == builds["provider"]["source_commit"] == result["source_commit"]
        assert builds["runtime"]["source_tree"] == builds["provider"]["source_tree"] == result["source_tree"]
        source_files = json.loads((output / "gateway-source-files.json").read_bytes())
        assert all(sha(helper_root / name) == digest for name, digest in source_files.items())
        frozen_files = json.loads((accepted / "receipts/source-files.json").read_bytes())
        assert all(sha(Path(os.environ["SEED_HELPER_ROOT"]) / name) == digest for name, digest in frozen_files.items())
        installed = {}
        for role in ("holder", "consumer"):
            home = root / role
            data = home / ".local/share/elastos"
            (home / ".local/bin").mkdir(mode=0o700, parents=True)
            (data / "bin").mkdir(mode=0o700, parents=True)
            runtime = home / ".local/bin/elastos"
            source = accepted / ("gateway-elastos" if role == "holder" else "elastos")
            shutil.copyfile(source, runtime)
            runtime.chmod(0o700)
            expected = builds["runtime"] if role == "holder" else accepted_runtime
            assert sha(runtime) == expected["sha256"] and runtime.stat().st_size == expected["size"]
            processes.roots[str(runtime)] = {sha(runtime)}
            manifest = {"external": {}, "profiles": {}, "capsules": {}}
            names = ("ipfs-provider", "kubo") if role == "holder" else ("localhost-provider",)
            for name in names:
                source = accepted / name if name != "kubo" else accepted / "kubo-input/bin/kubo"
                target = data / "bin" / name
                shutil.copyfile(source, target)
                target.chmod(0o700)
                assert sha(target) == sha(source)
                entry = json.loads(json.dumps(template["external"][name]))
                entry["platforms"] = {"linux-amd64": {**entry["platforms"]["linux-amd64"],
                    "checksum": "sha256:" + sha(target), "size": target.stat().st_size}}
                manifest["external"][name] = entry
                processes.roots[str(target)] = {sha(target)}
            (data / "components.json").write_text(json.dumps(manifest))
            (data / "components.json").chmod(0o600)
            (data / "config.toml").write_text('carrier_bind_addr = "127.0.0.1:' + str(port(socket.SOCK_DGRAM))
                + '"\ngateway_public_publisher_bootstrap = true\ngateway_allowed_hosts = ["127.0.0.1"]\n')
            (data / "config.toml").chmod(0o600)
            command(home, [str(runtime), "node", "info", "--json"], role + "-identity")
            installed[role] = home, data, runtime, port()
        home, data, runtime, gateway_port = installed["holder"]
        assert sha(data / "bin/ipfs-provider") == builds["provider"]["sha256"]
        kubo_env = {**environment(home), "IPFS_PATH": str(data / "ipfs-repo")}
        reply = processes.command([str(data / "bin/kubo"), "init", "--profile=test"], kubo_env, home,
                                  "always-on-kubo-init")
        assert reply["exit"] == 0
        config_path = data / "ipfs-repo/config"
        config = json.loads(config_path.read_bytes())
        config["Bootstrap"], config["Addresses"]["Swarm"] = [], []
        config["Routing"]["Type"], config["Discovery"]["MDNS"]["Enabled"] = "none", False
        config["AutoConf"]["Enabled"] = False
        config_path.write_text(json.dumps(config))
        warmup = root / "warmup.bin"
        warmup.write_bytes(b"issue89 gateway always-on fixture\n" * 4096)
        cids = []
        for label, content in (("warmup", warmup), ("release", accepted / "elastos")):
            reply = processes.command([str(data / "bin/kubo"), "add", "--quiet", "--cid-version=1", "--pin=true",
                                       str(content)], kubo_env, home, "always-on-" + label + "-add")
            assert reply["exit"] == 0
            cid = processes.text("always-on-" + label + "-add").strip()
            assert re.fullmatch(r"b[a-z2-7]+", cid)
            cids.append(cid)
        holder = processes.spawn([str(runtime), "gateway", "--addr", "127.0.0.1:" + str(gateway_port),
            "--cache-dir", str(home / "gateway-cache")], environment(home), home, "always-on-holder")
        ready(holder, f"http://127.0.0.1:{gateway_port}/healthz")
        auth = attach(data, holder, "gateway-runtime-coords.json", "gateway")
        ticket_reply = post(auth[0] + "/api/provider/peer/get_ticket", {}, auth[1])
        assert ticket_reply["status"] == "ok"
        ticket = ticket_reply["data"]["ticket"]
        assert isinstance(ticket, str) and re.fullmatch(r"[a-zA-Z2-7]+", ticket) and len(ticket) <= 65536
        decoded = json.loads(base64.b32decode(ticket.upper() + "=" * (-len(ticket) % 8)))
        assert len(decoded["endpoints"]) == 1
        assert decoded["endpoints"][0]["id"] == ticket_reply["data"]["node_id"]
        consumer_home, consumer_data, consumer_runtime, consumer_port = installed["consumer"]
        did = json.loads(processes.text("always-on-holder-identity"))["did"]
        command(consumer_home, [str(consumer_runtime), "node", "peer", "add", "--did", did,
            "--ticket", ticket, "--json"], "consumer-holder-peer")
        consumer = processes.spawn([str(consumer_runtime), "serve", "--addr", "127.0.0.1:" + str(consumer_port),
            "--storage-path", str(consumer_data / "storage")], environment(consumer_home), consumer_home,
            "always-on-consumer")
        ready(consumer, f"http://127.0.0.1:{consumer_port}/api/health")
        consumer_auth = attach(consumer_data, consumer, "runtime-coords.json", "operator")
        assert not any((consumer_data / name).exists() for name in ("bin/ipfs-provider", "bin/kubo", "ipfs-repo"))
        assert get(f"http://127.0.0.1:{gateway_port}/content/{cids[0]}", warmup.stat().st_size) == warmup.read_bytes()
        assert carrier_fetch(consumer_auth, cids[0], warmup) == warmup.read_bytes()
        assert "Runtime host role Gateway; idle stop disabled" in processes.text("always-on-holder", "stderr")
        holder_identity, consumer_identity = identity(holder.pid), identity(consumer.pid)
        matches = [row for row in observer.cli_census() if row["group"] == holder.pid
                   and row["command"].split(" ", 1)[0] == str(data / "bin/ipfs-provider")]
        assert len(matches) == 1
        provider_identity = identity(matches[0]["pid"])
        coord_path = data / "ipfs-coords.json"
        before = json.loads(coord_path.read_bytes())
        kubo_identity = identity(before["kubo_pid"])
        assert kubo_identity["exe"] == str(data / "bin/kubo")
        started = time.monotonic()
        # Observe local process/coord state only. Send no provider, HTTP, Carrier
        # or preparation requests until the first post-idle content fetch.
        while time.monotonic() - started < 665:
            assert holder.poll() is None and consumer.poll() is None
            assert identity(holder.pid) == holder_identity and identity(consumer.pid) == consumer_identity
            assert identity(provider_identity["pid"]) == provider_identity
            assert identity(kubo_identity["pid"]) == kubo_identity
            assert json.loads(coord_path.read_bytes()) == before, "idle coordinates changed"
            time.sleep(1)
        idle_elapsed = time.monotonic() - started
        idle_coord = json.loads(coord_path.read_bytes())
        assert idle_coord == before
        assert idle_elapsed > 600 and int(time.time()) - before["last_used"] > 600
        assert "Kubo idle for" not in processes.text("always-on-holder", "stderr")
        request_started = time.monotonic()
        payload = carrier_fetch(consumer_auth, cids[1], accepted / "elastos")
        elapsed = time.monotonic() - request_started
        assert len(payload) == accepted_runtime["size"] and hashlib.sha256(payload).hexdigest() == accepted_runtime["sha256"]
        assert elapsed <= 15, "first read exceeded immediate-read fixture bound"
        after = json.loads(coord_path.read_bytes())
        assert {key: value for key, value in after.items() if key != "last_used"} == {
            key: value for key, value in before.items() if key != "last_used"}
        assert identity(kubo_identity["pid"]) == kubo_identity
        assert identity(provider_identity["pid"]) == provider_identity
        assert identity(holder.pid) == holder_identity
        fetched = consumer_home / ".local/bin/carrier-fetched-elastos"
        fetched.write_bytes(payload)
        fetched.chmod(0o700)
        processes.roots[str(fetched)] = {sha(fetched)}
        version = command(consumer_home, [str(fetched), "--version"], "fetched-version")
        assert version == "elastos " + os.environ["ELASTOS_RELEASE_VERSION"]
        assert processes.text("always-on-fetched-version", "stderr") == ""
        result.update(passed=True, role="gateway", role_selected_by="Runtime gateway command private verified Init",
            idle_elapsed_seconds=idle_elapsed, last_used_before=before["last_used"], last_used_after_idle=idle_coord["last_used"],
            holder=holder_identity, provider=provider_identity, kubo=kubo_identity, same_kubo_generation=True,
            carrier_read={"cid": cids[1], "size": len(payload), "sha256": sha(fetched), "version": version,
                "elapsed_seconds": elapsed, "immediate_read_bound_seconds": 15,
                "transport": "typed availability / Carrier provider_invoke / holder Content / native IPFS cat",
                "legacy_content_fetch_journey": False, "consumer_local_backend": False},
            installed_runtime=builds["runtime"], installed_provider=builds["provider"],
            consumer_runtime=accepted_runtime, localhost_fixture=builds["localhost"],
            installed_kubo={"sha256":sha(data / "bin/kubo"), "size":(data / "bin/kubo").stat().st_size},
            installed_manifest_sha256=sha(data / "components.json"))
        for checkout in ("source", "provider-source", "observer"):
            assert not subprocess.check_output(["git", "-C", str(workspace / checkout), "status",
                "--porcelain=v1", "--untracked-files=all"], text=True).strip(), "source changed during qualification"
        assert all(sha(helper_root / name) == digest for name, digest in source_files.items())
        assert all(sha(Path(os.environ["SEED_HELPER_ROOT"]) / name) == digest for name, digest in frozen_files.items())
    except Exception as error:
        result["failure"] = {"type": type(error).__name__, "message": str(error)}
        raise
    finally:
        result["cleanup"] = processes.cleanup()
        if not result["cleanup"]["errors"] and result["cleanup"]["remaining_pids"] == []:
            shutil.rmtree(root)
            result["fixture_removed"] = True
            candidate_root = Path(os.environ["CI_GATEWAY_HELPER_ROOT"])
            assert str(candidate_root) == "/home/runner/.local/share/elastos-issue89-candidate/source"
            shutil.rmtree(candidate_root.parent)
            result["candidate_helper_removed"] = True
        else:
            result["passed"] = False
        (output / "holder-always-on.json").write_text(json.dumps(result, indent=2) + "\n")
        assert not result["cleanup"]["errors"] and result["cleanup"]["remaining_pids"] == [], "cleanup incomplete"


if __name__ == "__main__":
    main()
