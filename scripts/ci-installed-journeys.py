#!/usr/bin/env python3
"""Run journeys against this job's installed Home; save only public proof."""
import argparse
import base64
import hashlib
import json
import os
import re
import runpy
import select
import secrets
import signal
import shutil
import socket
import stat
import subprocess
import sys
import threading
import time
import urllib.request
from pathlib import Path

TIMING = runpy.run_path(str(Path(__file__).with_name("ci-installed-model-timing.py")))
UPSTREAM = runpy.run_path(str(Path(__file__).with_name("release-upstream-input.py")))
DISK_RESERVE_BYTES = 2 * 1024 ** 3


def bounded_text(path, limit=16384):
    with Path(path).open("rb") as source:
        value = source.read(limit + 1)
    if len(value) > limit:
        raise RuntimeError("finite journey observation exceeds its bound")
    return value.decode("utf-8")


def private_json(path, value=None):
    """Private, bounded one-owner control files; upload only the public receipt."""
    path = Path(path)
    if value is not None:
        encoded = json.dumps(value).encode()
        if len(encoded) > 65536:
            raise RuntimeError("finite journey control exceeds its bound")
        temporary = path.with_name(path.name + ".next")
        fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        with os.fdopen(fd, "wb") as output:
            output.write(encoded)
        os.replace(temporary, path)
        return value
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC)
    with os.fdopen(fd, "rb") as source:
        metadata = os.fstat(source.fileno())
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.getuid() or metadata.st_nlink != 1 or metadata.st_mode & 0o777 != 0o600:
            raise RuntimeError("finite journey control ownership differs")
        encoded = source.read(65537)
    if len(encoded) > 65536:
        raise RuntimeError("finite journey control exceeds its bound")
    return json.loads(encoded)


# Only fixed harness messages can leave the isolated Home. Other exception
# details can contain paths, keys or Runtime text and use one redacted message.
FINITE_OBSERVER_MESSAGES = frozenset({
    'finite journey Runtime left its unit',
    'finite journey actual Runtime identity differs',
    'finite journey cgroup membership is invalid',
    'finite journey child UID differs',
    'finite journey child escaped its finite unit',
    'finite journey child executable is outside installed artifacts',
    'finite journey control exceeds its bound',
    'finite journey control ownership differs',
    'finite journey engine group exit is unconfirmed',
    'finite journey engine stage deadline',
    'finite journey engine stage interrupted',
    'finite journey idle engine closed early',
    'finite journey kernel limits changed',
    'finite journey kernel limits differ',
    'finite journey live child identity is unavailable',
    'finite journey live child left its unit census',
    'finite journey live child observation is unavailable',
    'finite journey memory events are invalid',
    'finite journey memory events are unavailable',
    'finite journey memory exceeded its kernel boundary',
    'finite journey memory metric exceeds its bound',
    'finite journey observation exceeds its bound',
    'finite journey observed a second local engine',
    'finite journey observed an OOM event',
    'finite journey observed process bound exceeded',
    'finite journey observer deadline',
    'finite journey observer, holder or browser entered the unit',
    'finite journey outside process identity is unavailable',
    'finite journey process census differs',
    'finite journey process changed during pidfd binding',
    'finite journey process exit is unavailable',
    'finite journey process identity changed',
    'finite journey process identity is unavailable',
    'finite journey required ancestor memory bound differs',
    'finite journey requires runner UID and a 1 or 4 GiB budget',
    'finite journey requires visible cgroup v2 membership',
    'finite journey selected engine is unavailable',
    'finite journey service binding differs',
    'finite journey service observation exceeds its bound',
    'finite journey stage binding differs',
    'finite journey stage is unsupported',
    'finite journey starts with an OOM event',
})


def finite_observer_failure(phase, error):
    message = (error.args[0] if type(error) is RuntimeError and len(error.args) == 1
               and type(error.args[0]) is str else None)
    if message not in FINITE_OBSERVER_MESSAGES:
        message = "observer detail redacted"
    public_types = {
        RuntimeError: "RuntimeError", OSError: "OSError", FileNotFoundError: "FileNotFoundError",
        PermissionError: "PermissionError", ProcessLookupError: "ProcessLookupError",
        ValueError: "ValueError", KeyError: "KeyError", TypeError: "TypeError",
        AttributeError: "AttributeError", UnicodeDecodeError: "UnicodeDecodeError",
        json.JSONDecodeError: "JSONDecodeError", subprocess.SubprocessError: "SubprocessError",
        subprocess.CalledProcessError: "CalledProcessError", subprocess.TimeoutExpired: "TimeoutExpired",
    }
    return {"phase": phase, "type": public_types.get(type(error), "redacted"), "message": message[:480]}


def failure_suffix(failure):
    return f" ({failure['phase']}: {failure['type']}: {failure['message']})" if failure else ""


def linux_process_identity(pid, proc=Path("/proc"), retained=None):
    """Bind a live process by UID, kernel start tick and executable inode."""
    directory = proc / str(pid)
    stat = bounded_text(directory / "stat", 4096)
    fields = stat[stat.rfind(")") + 2:].split()
    status = bounded_text(directory / "status")
    uid = next(line for line in status.splitlines() if line.startswith("Uid:")).split()[1:]
    if len(fields) < 20 or len(uid) != 4 or len(set(uid)) != 1:
        raise RuntimeError("finite journey process identity is unavailable")
    identity = {"pid": int(pid), "uid": int(uid[0]), "start_ticks": int(fields[19]),
                "parent": int(fields[1]), "group": int(fields[2]), "state": fields[0]}
    try:
        executable = (directory / "exe").resolve(strict=True)
        metadata = executable.stat()
        identity.update(executable=str(executable), device=metadata.st_dev, inode=metadata.st_ino)
    except FileNotFoundError:
        # A dead main thread can leave live siblings. The caller retains its
        # flags=0 pidfd and checks whole-process exit; state Z is no exit proof.
        if identity["state"] != "Z" or retained is None or not same_birth(identity, retained):
            raise
        identity.update({key: retained[key] for key in ("executable", "device", "inode")})
    return identity


def same_birth(left, right):
    return all(left.get(key) == right.get(key) for key in ("pid", "uid", "start_ticks"))


def same_process(left, right):
    return all(left.get(key) == right.get(key) for key in
               ("pid", "uid", "start_ticks", "executable", "device", "inode"))


def pidfd_exited(fd):
    poll = select.poll()
    poll.register(fd, select.POLLIN)
    events = poll.poll(0)
    # Reaped processes can also report HUP. Readability still proves the
    # flags=0 pidfd's whole thread group exited; HUP alone proves no exit.
    if events not in ([], [(fd, select.POLLIN)], [(fd, select.POLLIN | select.POLLHUP)]):
        raise RuntimeError("finite journey process exit is unavailable")
    return bool(events)


def cgroup_membership(pid, proc=Path("/proc")):
    rows = bounded_text(proc / str(pid) / "cgroup", 4096).splitlines()
    if len(rows) != 1 or not rows[0].startswith("0::/"):
        raise RuntimeError("finite journey requires visible cgroup v2 membership")
    path = rows[0][3:]
    parts = path[1:].split("/") if path != "/" else []
    if len(path) > 1024 or len(parts) > 32 or any(not part or part in (".", "..") or len(part) > 255 or any(ord(c) < 32 for c in part) for part in parts):
        raise RuntimeError("finite journey cgroup membership is invalid")
    return path


def finite_memory_preflight(platform, proc=Path("/proc"), root=Path("/sys/fs/cgroup")):
    if platform != "linux" or not hasattr(os, "pidfd_open") or not hasattr(signal, "pidfd_send_signal"):
        raise RuntimeError("finite installed journey requires Linux pidfds")
    mount_text = bounded_text(proc / "self/mountinfo", 65536)
    mounts, ancestors, groups = mount_text.splitlines(), {}, 0
    if len(mounts) > 1024:
        raise RuntimeError("finite journey mount census exceeds its bound")
    def path_field(value):
        escapes = {"040": " ", "011": "\t", "012": "\n", "134": "\\"}
        if re.search(r"\\(?!040|011|012|134)", value):
            raise RuntimeError("finite journey mount path is invalid")
        return Path(re.sub(r"\\(040|011|012|134)", lambda found: escapes[found[1]], value))
    for line in mounts:
        fields = line.split()
        split = fields.index("-")
        if len(fields) > 64 or split < 6 or len(fields) != split + 4 or not fields[0].isdecimal() or not fields[1].isdecimal():
            raise RuntimeError("finite journey mount record is invalid")
        mountpoint = path_field(fields[4])
        if fields[split + 1] == "cgroup2":
            groups += 1
            if path_field(fields[3]) != Path("/") or mountpoint != root:
                raise RuntimeError("finite journey requires the global cgroup v2 mount")
        elif mountpoint == root or mountpoint.is_relative_to(root):
            raise RuntimeError("finite journey cgroup mount is covered")
        if root == mountpoint or root.is_relative_to(mountpoint):
            identity = (int(fields[0]), int(fields[1]))
            if mountpoint in ancestors or any(item[0] == identity[0] for item in ancestors.values()):
                raise RuntimeError("finite journey mount ancestry is ambiguous")
            ancestors[mountpoint] = identity
    path, visited = root, set()
    while path != Path("/"):
        if path in visited or path not in ancestors:
            raise RuntimeError("finite journey mount ancestry is hidden")
        visited.add(path)
        parent = ancestors[path][1]
        candidate = [point for point, identity in ancestors.items() if identity[0] == parent]
        if len(candidate) != 1 or candidate[0] == path or not path.is_relative_to(candidate[0]):
            raise RuntimeError("finite journey mount ancestry is hidden")
        path = candidate[0]
    if groups != 1 or path not in ancestors or len(visited) + 1 != len(ancestors):
        raise RuntimeError("finite journey requires visible global mount ancestry")
    if "memory" not in bounded_text(root / "cgroup.controllers", 4096).split():
        raise RuntimeError("finite journey requires the memory controller")
    for name in ("cgroup.type", "memory.max", "memory.current"):
        try:
            bounded_text(root / name, 128)
        except FileNotFoundError:
            continue
        raise RuntimeError("finite journey requires the visible global cgroup root")
    membership = cgroup_membership("self", proc)
    if mount_text != bounded_text(proc / "self/mountinfo", 65536) or membership != cgroup_membership("self", proc):
        raise RuntimeError("finite journey host visibility changed")
    return membership


def runtime_unit_helper(control):
    """The unit's helper is the sole Popen/reap owner of its Runtime child."""
    payload = private_json(control / "launch.json")
    parent = linux_process_identity(payload["owner"]["pid"])
    if not same_process(parent, payload["owner"]):
        raise RuntimeError("finite journey owner changed before launch")
    owner_fd = os.pidfd_open(parent["pid"], 0)
    try:
        if not same_process(parent, linux_process_identity(parent["pid"])) or pidfd_exited(owner_fd):
            raise RuntimeError("finite journey owner is unavailable")
        installed = Path(payload["runtime"])
        if digest(installed) != payload["runtime_sha256"]:
            raise RuntimeError("finite journey Runtime bytes changed")
        stopping = threading.Event()
        releasing = threading.Event()
        def terminate(*_):
            stopping.set()
            releasing.set()
        signal.signal(signal.SIGTERM, terminate)
        signal.signal(signal.SIGINT, terminate)
        log_fd = os.open(payload["log"], os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        with os.fdopen(log_fd, "wb") as log:
            child = subprocess.Popen([str(installed), "gateway", "--addr", payload["address"]],
                                     env=payload["environment"], stdout=log, stderr=log, start_new_session=True)
            try:
                identity = linux_process_identity(child.pid)
                private_json(control / "runtime.json", identity)
                deadline = time.monotonic() + 1800
                while not stopping.is_set() and time.monotonic() < deadline:
                    if (control / "stop.json").exists() or pidfd_exited(owner_fd) or child.poll() is not None:
                        break
                    stopping.wait(.1)
            finally:
                # This direct parent reserves its Popen PID until wait returns.
                if child.poll() is None:
                    child.terminate()
                try:
                    child.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait(timeout=5)
                private_json(control / "runtime-stopped.json", {"reaped": True})
                # Hold the unit's kernel counters until its outside owner reads
                # them. systemd prunes an empty RemainAfterExit service cgroup.
                deadline = time.monotonic() + 60
                while not releasing.is_set() and time.monotonic() < deadline and not pidfd_exited(owner_fd):
                    if (control / "release.json").exists():
                        if private_json(control / "release.json") != {"release": True}:
                            raise RuntimeError("finite journey helper release differs")
                        break
                    releasing.wait(.1)
    finally:
        os.close(owner_fd)


class FiniteRuntime:
    """Outside observer for one identity-bound transient CI service."""
    def __init__(self, home, data, address, environment, gib, platform=sys.platform):
        self.outside_group = finite_memory_preflight(platform)
        if gib not in (1, 4) or os.getuid() == 0:
            raise RuntimeError("finite journey requires runner UID and a 1 or 4 GiB budget")
        self.home, self.data, self.gib = home, data, gib
        self.budget = gib * 1024 ** 3
        self.unit = f"elastos-ci-model-{os.getpid()}-{secrets.token_hex(8)}.service"
        self.control = home / "finite-control"
        self.control.mkdir(mode=0o700)
        self.known, self.fds, self.actions = {}, {}, []
        self.metric_fds = {}
        self.stop = threading.Event()
        self.error = None
        self.failure = None
        self.thread = None
        self.outside = [os.getpid()]
        self.engine = None
        self.engine_members = set()
        self.last_action = 0
        self.started = time.monotonic()
        self.binding = None
        self.launch_requested = False
        self.pid = None
        self.baseline_events = None
        allowed = ("HOME", "XDG_DATA_HOME", "PATH", "TMPDIR", "LANG", "LC_ALL", "ELASTOS_MODEL_TIMING_DIAGNOSTICS")
        private_json(self.control / "launch.json", {
            "owner": linux_process_identity(os.getpid()), "runtime": str(data / "bin/elastos"),
            "runtime_sha256": digest(data / "bin/elastos"), "address": address,
            "environment": {key: environment[key] for key in allowed if key in environment},
            "log": str(home / "journey-runtime.private.log"),
        })

    def command(self, *args):
        result = subprocess.run(["sudo", "-n", *args], check=True, text=True,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=40)
        if len(result.stdout) > 65536:
            raise RuntimeError("finite journey service observation exceeds its bound")
        return result.stdout

    def show(self):
        text = self.command("systemctl", "show", self.unit, "--property=InvocationID,ControlGroup,MainPID,ActiveState")
        return dict(line.split("=", 1) for line in text.splitlines() if "=" in line)

    def start(self):
        self.launch_requested = True
        self.command("systemd-run", "--quiet", "--unit=" + self.unit, "--uid=" + str(os.getuid()),
                     "--gid=" + str(os.getgid()), "--property=Type=exec", "--property=MemoryAccounting=yes",
                     "--property=MemoryMax=" + str(self.budget), "--property=MemorySwapMax=0",
                     "--property=TasksMax=512", "--property=KillMode=control-group", "--property=TimeoutStopSec=30",
                     "--property=RemainAfterExit=yes",
                     "--property=WorkingDirectory=" + str(Path.cwd()),
                     str(Path(sys.executable).resolve()), str(Path(__file__).resolve()),
                     "--runtime-unit-helper", str(self.control))
        self.binding = self.show()
        if (not re.fullmatch("[0-9a-f]{32}", self.binding.get("InvocationID", "")) or self.binding.get("ActiveState") != "active"
                or self.binding.get("ControlGroup") != "/system.slice/" + self.unit):
            raise RuntimeError("finite journey service binding differs")
        self.group = Path("/sys/fs/cgroup") / self.binding["ControlGroup"].lstrip("/")
        deadline = time.monotonic() + 30
        while not (self.control / "runtime.json").exists() and time.monotonic() < deadline:
            time.sleep(.1)
        self.runtime = private_json(self.control / "runtime.json")
        identity = linux_process_identity(self.runtime["pid"])
        if not same_process(identity, self.runtime) or identity["parent"] != int(self.binding["MainPID"]):
            raise RuntimeError("finite journey actual Runtime identity differs")
        self.pid = identity["pid"]
        for name in ("memory.max", "memory.swap.max", "pids.max", "memory.current", "memory.peak", "memory.events", "cgroup.events"):
            self.metric_fds[name] = os.open(self.group / name, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
        if (self.metric("memory.max") != str(self.budget) or self.metric("memory.swap.max") != "0"
                or self.metric("pids.max") != "512"):
            raise RuntimeError("finite journey kernel limits differ")
        ancestor = self.group
        while ancestor != Path("/sys/fs/cgroup"):
            maximum = bounded_text(ancestor / "memory.max", 128).strip()
            current = bounded_text(ancestor / "memory.current", 128).strip()
            if not current.isdecimal() or maximum != "max" and (not maximum.isdecimal() or int(maximum) < self.budget):
                raise RuntimeError("finite journey required ancestor memory bound differs")
            ancestor = ancestor.parent
        self.baseline_events = self.events()
        if self.baseline_events.get("oom", 0) or self.baseline_events.get("oom_kill", 0):
            raise RuntimeError("finite journey starts with an OOM event")
        self.observe_once()
        self.thread = threading.Thread(target=self.observe, daemon=True)
        self.thread.start()
        return self

    def metric(self, name):
        fd = self.metric_fds[name]
        os.lseek(fd, 0, os.SEEK_SET)
        value = os.read(fd, 4097)
        if len(value) > 4096:
            raise RuntimeError("finite journey memory metric exceeds its bound")
        return value.decode().strip()

    def events(self):
        rows = [row.split() for row in self.metric("memory.events").splitlines()]
        if any(len(row) != 2 or not row[1].isdecimal() for row in rows):
            raise RuntimeError("finite journey memory events are invalid")
        result = {key: int(value) for key, value in rows}
        if len(result) != len(rows) or not {"oom", "oom_kill"}.issubset(result):
            raise RuntimeError("finite journey memory events are unavailable")
        return result

    def poll(self):
        return 1 if self.error or self.pid in self.fds and pidfd_exited(self.fds[self.pid]) else None

    def observe_once(self, require_runtime=True):
        if (self.metric("memory.max") != str(self.budget) or self.metric("memory.swap.max") != "0"
                or self.metric("pids.max") != "512"):
            raise RuntimeError("finite journey kernel limits changed")
        pids = [int(value) for value in bounded_text(self.group / "cgroup.procs", 8192).split()]
        if not pids or len(pids) > 512 or len(set(pids)) != len(pids):
            raise RuntimeError("finite journey process census differs")
        for pid in list(self.outside):
            try:
                membership = cgroup_membership(pid)
            except FileNotFoundError:
                # The direct owner can reap its browser between census ticks.
                # A missing leader alone is insufficient while siblings live.
                try:
                    fd = os.pidfd_open(pid, 0)
                except ProcessLookupError:
                    continue
                try:
                    if not pidfd_exited(fd):
                        raise RuntimeError("finite journey outside process identity is unavailable")
                finally:
                    os.close(fd)
                continue
            if membership == self.binding["ControlGroup"] or membership.startswith(self.binding["ControlGroup"] + "/"):
                raise RuntimeError("finite journey observer, holder or browser entered the unit")
        current = {}
        for pid in pids:
            retained = self.known.get(pid)
            try:
                identity = linux_process_identity(pid, retained=retained)
            except FileNotFoundError:
                if pid in self.fds and pidfd_exited(self.fds[pid]):
                    continue
                try:
                    fd = os.pidfd_open(pid, 0)
                except ProcessLookupError:
                    continue
                try:
                    if pidfd_exited(fd):
                        continue
                finally:
                    os.close(fd)
                raise RuntimeError("finite journey live child identity is unavailable")
            if identity["uid"] != os.getuid():
                raise RuntimeError("finite journey child UID differs")
            if retained and not same_birth(retained, identity):
                raise RuntimeError("finite journey process identity changed")
            if pid not in self.fds:
                if len(self.fds) >= 512:
                    raise RuntimeError("finite journey observed process bound exceeded")
                try:
                    fd = os.pidfd_open(pid, 0)
                except ProcessLookupError:
                    continue
                try:
                    if pidfd_exited(fd):
                        os.close(fd)
                        continue
                    try:
                        rebound = linux_process_identity(pid, retained=identity)
                    except FileNotFoundError:
                        if pidfd_exited(fd):
                            os.close(fd)
                            continue
                        raise
                    if not same_process(identity, rebound):
                        raise RuntimeError("finite journey process changed during pidfd binding")
                except BaseException:
                    os.close(fd)
                    raise
                self.fds[pid] = fd
            elif pidfd_exited(self.fds[pid]):
                # A reused PID cannot become another owned process under this FD.
                continue
            try:
                membership = cgroup_membership(pid)
                command = bounded_text(Path("/proc") / str(pid) / "cmdline", 8192).split("\0")
            except FileNotFoundError:
                if pidfd_exited(self.fds[pid]):
                    continue
                raise RuntimeError("finite journey live child observation is unavailable")
            if membership != self.binding["ControlGroup"]:
                raise RuntimeError("finite journey child escaped its finite unit")
            executable = Path(identity["executable"])
            native = any(executable.is_relative_to(self.data / name) for name in ("bin", "libexec"))
            if not native and (pid != int(self.binding["MainPID"]) or executable != Path(sys.executable).resolve()):
                raise RuntimeError("finite journey child executable is outside installed artifacts")
            if retained is None or not same_process(retained, identity):
                identity["exe_sha256"] = digest(Path(identity["executable"]))
                identity["role"] = ("runtime" if pid == self.pid else
                                    "helper" if pid == int(self.binding["MainPID"]) else
                                    "guard" if "--internal-local-llama-guard" in command else
                                    "model_provider" if executable == (self.data / "bin/model-provider").resolve() else
                                    "llama_server" if executable.name == "llama-server" else "other")
                self.known[pid] = identity
            current[pid] = self.known[pid]
        if any(pid not in current and not pidfd_exited(fd) for pid, fd in self.fds.items()):
            raise RuntimeError("finite journey live child left its unit census")
        if require_runtime and self.pid not in current:
            raise RuntimeError("finite journey Runtime left its unit")
        memory = [self.metric(name) for name in ("memory.current", "memory.peak")]
        if any(not value.isdecimal() or int(value) > self.budget for value in memory):
            raise RuntimeError("finite journey memory exceeded its kernel boundary")
        if any(self.events().get(key, 0) != self.baseline_events.get(key, 0) for key in ("oom", "oom_kill")):
            raise RuntimeError("finite journey observed an OOM event")
        return current

    def engines(self, current):
        _, _, info, bundle = engine_paths(self.data)
        executable = str((bundle / info["binary_path"]).resolve())
        provider = str((self.data / "bin/model-provider").resolve())
        found = []
        for pid, row in current.items():
            guard = current.get(row["parent"])
            adapter = current.get(guard["parent"]) if guard else None
            if row["executable"] == executable and guard and adapter and guard.get("role") == "guard" and guard["executable"] == provider and adapter["executable"] == provider and adapter["parent"] == self.pid:
                found.append(pid)
        if len(found) > 1:
            raise RuntimeError("finite journey observed a second local engine")
        return found

    def action(self, request):
        if len(self.actions) >= 16 or set(request) != {"id", "action"} or type(request["id"]) is not int or request["id"] != self.last_action + 1:
            raise RuntimeError("finite journey stage binding differs")
        action = request["action"]
        if action not in ("engine_running", "engine_stopped", "idle_stopped", "crash_engine"):
            raise RuntimeError("finite journey stage is unsupported")
        deadline = time.monotonic() + (95 if action == "idle_stopped" else 20)
        began = time.monotonic()
        group_exit_pending = False
        while time.monotonic() < deadline:
            if self.stop.is_set():
                raise RuntimeError("finite journey engine stage interrupted")
            current = self.observe_once()
            engines = self.engines(current)
            group_exit_pending = False
            if action == "engine_running" and engines:
                pid = engines[0]
                self.engine = pid
                group = current[pid]["group"]
                self.engine_members = {member for member, row in current.items() if row["group"] == group}
                break
            if action == "crash_engine":
                if engines != [self.engine] or pidfd_exited(self.fds[self.engine]):
                    raise RuntimeError("finite journey selected engine is unavailable")
                signal.pidfd_send_signal(self.fds[self.engine], signal.SIGKILL)
                break
            if action in ("engine_stopped", "idle_stopped") and not engines:
                if (self.engine is None or self.engine not in self.engine_members
                        or any(pid not in self.fds for pid in self.engine_members)):
                    raise RuntimeError("finite journey engine group exit is unconfirmed")
                # Engine exit before the idle minimum remains a failure even
                # when its guard takes longer to exit. Parent-chain absence
                # alone does not prove the retained engine process has exited.
                if (action == "idle_stopped" and pidfd_exited(self.fds[self.engine])
                        and time.monotonic() - began < 50):
                    raise RuntimeError("finite journey idle engine closed early")
                group_exit_pending = any(not pidfd_exited(self.fds[pid]) for pid in self.engine_members)
                # An absent engine can precede whole-group exit. Keep observing
                # its retained handles within this action's existing deadline.
                if not group_exit_pending:
                    if action == "idle_stopped" and time.monotonic() - began < 50:
                        raise RuntimeError("finite journey idle engine closed early")
                    break
            self.stop.wait(.1)
        else:
            if group_exit_pending:
                raise RuntimeError("finite journey engine group exit is unconfirmed")
            raise RuntimeError("finite journey engine stage deadline")
        self.last_action = request["id"]
        self.actions.append({"action": action, "elapsed_seconds": round(time.monotonic() - began, 3),
                             "engine_pid": self.engine, "engine_start_ticks": self.known[self.engine]["start_ticks"]})
        private_json(self.control / "response.json", {"id": self.last_action, "status": "passed", "engine_start_ticks": self.known[self.engine]["start_ticks"], "engine_pid": self.engine})

    def observe(self):
        phase = "census"
        try:
            while not self.stop.is_set() and time.monotonic() - self.started < 1800:
                phase = "census"
                self.observe_once()
                path = self.control / "request.json"
                if path.exists():
                    phase = "stage_request"
                    request = private_json(path)
                    if request.get("id") != self.last_action:
                        if request.get("action") in ("engine_running", "engine_stopped", "idle_stopped", "crash_engine"):
                            phase = "stage_" + request["action"]
                        self.action(request)
                self.stop.wait(.1)
            if not self.stop.is_set():
                phase = "deadline"
                raise RuntimeError("finite journey observer deadline")
        except BaseException as error:
            self.failure = finite_observer_failure(phase, error)
            self.error = "finite_resource_observation_failed"
            private_json(self.control / "response.json", {"id": self.last_action + 1, "status": "failed"})
            private_json(self.control / "stop.json", {"stop": True})

    def finish(self):
        self.stop.set()
        if self.thread:
            self.thread.join(timeout=3)
        record = {"limit_bytes": self.budget, "swap_limit_bytes": 0, "tasks_limit": 512,
                  "status": "failed", "cgroup_empty": False,
                  "observer_complete": self.thread is None or not self.thread.is_alive(),
                  "actions": self.actions, "outside_runtime_unit": True}
        if self.failure:
            record["observer_failure"] = self.failure
        phase = "unit_binding"
        try:
            if not self.launch_requested:
                return record
            current = self.show()
            if self.binding is None:
                # systemd-run may create its unit before an exec/startup fault.
                if not re.fullmatch("[0-9a-f]{32}", current.get("InvocationID", "")) or current.get("ControlGroup") != "/system.slice/" + self.unit:
                    return record
                self.binding = current
            if current.get("InvocationID") != self.binding["InvocationID"] or current.get("ControlGroup") != self.binding["ControlGroup"]:
                raise RuntimeError("finite journey unit identity changed before cleanup")
            try:
                phase = "observer_stop"
                if not record["observer_complete"]:
                    raise RuntimeError("finite journey observer did not stop before cleanup")
                phase = "runtime_reap"
                private_json(self.control / "stop.json", {"stop": True})
                deadline = time.monotonic() + 25
                while not (self.control / "runtime-stopped.json").exists() and time.monotonic() < deadline:
                    time.sleep(.1)
                reaped = private_json(self.control / "runtime-stopped.json") == {"reaped": True}
                if len(self.metric_fds) != 7 or self.baseline_events is None:
                    return record
                if not reaped:
                    raise RuntimeError("finite journey Runtime reap is unconfirmed")
                helper = int(self.binding["MainPID"])
                if self.known.get(helper, {}).get("role") != "helper" or helper not in self.fds:
                    raise RuntimeError("finite journey helper identity is unavailable")
                phase = "native_cleanup"
                # The helper remains alive while native children exit. Bind any
                # remaining child through the same census before signalling it.
                for action in (signal.SIGTERM, signal.SIGKILL):
                    current = self.observe_once(require_runtime=False)
                    native = {pid for pid in current if pid != helper}
                    if not native:
                        break
                    binding = self.show()
                    if any(binding.get(key) != self.binding[key] for key in ("InvocationID", "ControlGroup")):
                        raise RuntimeError("finite journey unit cleanup binding differs")
                    for pid in native:
                        if not pidfd_exited(self.fds[pid]):
                            try:
                                signal.pidfd_send_signal(self.fds[pid], action)
                            except ProcessLookupError:
                                if not pidfd_exited(self.fds[pid]):
                                    raise
                    deadline = time.monotonic() + (15 if action == signal.SIGTERM else 5)
                    while time.monotonic() < deadline:
                        if set(self.observe_once(require_runtime=False)) == {helper}:
                            break
                        time.sleep(.1)
                if (set(self.observe_once(require_runtime=False)) != {helper}
                        or any(not pidfd_exited(fd) for pid, fd in self.fds.items() if pid != helper)
                        or pidfd_exited(self.fds[helper])):
                    raise RuntimeError("finite journey native process exit is unconfirmed")
                phase = "kernel_snapshot"
                events = self.events()
                no_oom = all(events.get(key, 0) == self.baseline_events.get(key, 0) for key in ("oom", "oom_kill"))
                peak = int(self.metric("memory.peak"))
                if (self.metric("memory.max") != str(self.budget) or self.metric("memory.swap.max") != "0"
                        or self.metric("pids.max") != "512"):
                    raise RuntimeError("finite journey final kernel limits differ")
                record.update(memory_peak_bytes=peak, memory_events_before=self.baseline_events, memory_events_after=events,
                              runtime_reaped=reaped,
                              control_group=self.binding["ControlGroup"], native_membership_verified=True,
                              processes=[{key: row[key] for key in ("pid", "uid", "start_ticks", "exe_sha256", "role")} for row in self.known.values()],
                              actual_runtime_pid=self.pid, helper_main_pid=int(self.binding["MainPID"]))
                phase = "helper_release"
                private_json(self.control / "release.json", {"release": True})
                deadline = time.monotonic() + 5
                while not pidfd_exited(self.fds[helper]) and time.monotonic() < deadline:
                    time.sleep(.1)
                cleanup = all(pidfd_exited(fd) for fd in self.fds.values())
                binding = self.show()
                if (binding.get("InvocationID") != self.binding["InvocationID"]
                        or binding.get("ControlGroup") not in (self.binding["ControlGroup"], "")):
                    raise RuntimeError("finite journey unit identity changed after release")
                # An empty exited service can already have lost its cgroup and
                # metric files. Every observed whole process must also be gone.
                empty = not self.group.exists()
                if not empty and binding.get("ControlGroup") == self.binding["ControlGroup"]:
                    try:
                        empty = dict(row.split() for row in self.metric("cgroup.events").splitlines()).get("populated") == "0"
                    except OSError:
                        if self.group.exists():
                            raise
                        empty = True
                record.update(cgroup_empty=empty, whole_process_exit=cleanup)
            finally:
                # Stop only this bound unit. This also closes native descendants
                # if Runtime startup, UI, or the observer failed part way through.
                observed_phase, phase = phase, "unit_stop"
                binding = self.show()
                removed = (binding.get("ControlGroup") == "" and hasattr(self, "group") and not self.group.exists()
                           and all(pidfd_exited(fd) for fd in self.fds.values()))
                if (binding.get("InvocationID") != self.binding["InvocationID"]
                        or binding.get("ControlGroup") != self.binding["ControlGroup"] and not removed):
                    raise RuntimeError("finite journey unit was replaced before stop")
                self.command("systemctl", "stop", self.unit)
                phase = observed_phase
            if cleanup and empty and reaped and no_oom and peak <= self.budget and record["observer_complete"] and not self.error:
                record["status"] = "passed"
        except (OSError, ValueError, KeyError, RuntimeError, subprocess.SubprocessError):
            record["status"] = "failed"
            record["failure_phase"] = phase
        finally:
            for fd in (*self.fds.values(), *self.metric_fds.values()):
                os.close(fd)
        return record


def finite_receipt_valid(row, gib):
    """One acceptance predicate for the uploaded finite-memory receipts."""
    if not isinstance(row, dict) or type(gib) is not int or gib not in (1, 4):
        return False
    finite = row.get("finite_memory", {})
    if not isinstance(finite, dict) or not isinstance(row.get("results"), dict):
        return False
    runtime, helper = finite.get("actual_runtime_pid"), finite.get("helper_main_pid")
    peak = finite.get("memory_peak_bytes")
    processes, actions = finite.get("processes"), finite.get("actions")
    group = finite.get("control_group")
    if (not isinstance(group, str) or not re.fullmatch(r"/system.slice/elastos-ci-model-[a-z0-9-]+\.service", group)
            or finite.get("native_membership_verified") is not True
            or any(type(value) is not int or value <= 0 for value in (runtime, helper, peak))
            or runtime == helper or not isinstance(processes, list) or not 2 <= len(processes) <= 512
            or not isinstance(actions, list) or len(actions) > 16):
        return False
    seen = {}
    for process in processes:
        if not isinstance(process, dict) or set(process) != {"pid", "uid", "start_ticks", "exe_sha256", "role"}:
            return False
        if (type(process["pid"]) is not int or process["pid"] <= 0 or process["pid"] in seen
                or type(process["uid"]) is not int or process["uid"] != os.getuid()
                or type(process["start_ticks"]) is not int or process["start_ticks"] <= 0
                or not isinstance(process["exe_sha256"], str) or not re.fullmatch("[0-9a-f]{64}", process["exe_sha256"])
                or process["role"] not in ("runtime", "helper", "guard", "model_provider", "llama_server", "other")):
            return False
        seen[process["pid"]] = process
    if (seen.get(runtime, {}).get("role") != "runtime" or seen.get(helper, {}).get("role") != "helper"
            or seen[runtime]["exe_sha256"] != row.get("installed_runtime_sha256")):
        return False
    installed_engine = row.get("installed_engine")
    if not isinstance(installed_engine, dict):
        return False
    for process in processes:
        role = process["role"]
        expected = (row.get("installed_model_provider_sha256") if role in ("guard", "model_provider") else
                    installed_engine.get("executable_sha256") if role == "llama_server" else process["exe_sha256"])
        if process["exe_sha256"] != expected:
            return False
    if (finite.get("status") != "passed" or type(finite.get("limit_bytes")) is not int or finite.get("limit_bytes") != gib * 1024 ** 3
            or type(finite.get("swap_limit_bytes")) is not int or finite.get("swap_limit_bytes") != 0 or finite.get("tasks_limit") != 512
            or not all(finite.get(key) is True for key in
                       ("cgroup_empty", "whole_process_exit", "runtime_reaped", "observer_complete", "outside_runtime_unit", "browser_closed"))
            or peak > gib * 1024 ** 3):
        return False
    before, after = finite.get("memory_events_before", {}), finite.get("memory_events_after", {})
    if not isinstance(before, dict) or not isinstance(after, dict) or any(type(value.get(key)) is not int or value[key] != 0 for value in (before, after) for key in ("oom", "oom_kill")):
        return False
    for action in actions:
        if not isinstance(action, dict) or set(action) != {"action", "elapsed_seconds", "engine_pid", "engine_start_ticks"}:
            return False
        if type(action["engine_pid"]) is not int or type(action["engine_start_ticks"]) is not int:
            return False
        engine = seen.get(action["engine_pid"], {})
        if (engine.get("role") != "llama_server" or engine.get("start_ticks") != action["engine_start_ticks"]
                or type(action["elapsed_seconds"]) not in (int, float) or not 0 <= action["elapsed_seconds"] <= 100):
            return False
    if gib == 1:
        return (row.get("results", {}).get("memory_refusal") == "passed"
                and row.get("results", {}).get("finite_home_recovery") == "passed"
                and actions == []
                and not any(value.get("role") in ("guard", "llama_server") for value in finite.get("processes", [])))
    return (all(row.get("results", {}).get(name) == "passed" for name in
                ("model_busy", "model_stop", "model_stop_recovery", "model_idle_release", "model_idle_recovery",
                 "model_engine_crash", "model_crash_recovery", "finite_home_recovery"))
            and [value.get("action") for value in finite.get("actions", [])] ==
                ["engine_running", "engine_stopped", "engine_running", "idle_stopped", "engine_running",
                 "engine_running", "crash_engine", "engine_stopped", "engine_running"])


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


def free_port(kind=socket.SOCK_STREAM):
    with socket.socket(socket.AF_INET, kind) as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def wait_healthy(child, address, label, stop=None):
    deadline = time.monotonic() + 120
    while time.monotonic() < deadline and not (stop and stop.is_set()):
        assert child.poll() is None, f"installed {label} exited before health" + failure_suffix(getattr(child, "failure", None))
        try:
            with urllib.request.urlopen(f"http://{address}/healthz", timeout=1) as response:
                if response.status == 200:
                    return
        except OSError:
            time.sleep(0.25)
    raise RuntimeError(f"installed {label} health deadline")


def post_json(url, body, token=None):
    headers = {"Content-Type": "application/json"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    request = urllib.request.Request(url, json.dumps(body).encode(), headers)
    with urllib.request.urlopen(request, timeout=60) as response:
        return json.loads(response.read(1024 * 1024))


def start_holder(home, data, consumer_data, consumer_environment):
    """Start the separate holder Home and name it as the consumer's one Carrier peer.

    The holder gateway starts its own Kubo with the pinned package before health.
    """
    environment = dict(os.environ, HOME=str(home), XDG_DATA_HOME=str(data.parent))
    installed, number = str(data / "bin/elastos"), free_port()
    with (home / "holder-runtime.private.log").open("wb") as log:
        child = subprocess.Popen([installed, "gateway", "--addr", f"127.0.0.1:{number}"], env=environment,
                                 stdout=log, stderr=log, start_new_session=True)
    try:
        wait_healthy(child, f"localhost:{number}", "holder Runtime")
        deadline = time.monotonic() + 30
        while not (data / "gateway-runtime-coords.json").is_file() and time.monotonic() < deadline:
            time.sleep(0.25)
        coords = json.loads((data / "gateway-runtime-coords.json").read_text())
        api = coords["api_url"].rstrip("/")
        token = post_json(api + "/api/auth/attach", {"secret": coords["attach_secret"], "scope": "shell"})["token"]
        ticket = post_json(api + "/api/provider/peer/get_ticket", {}, token)["data"]["ticket"]
        did = json.loads(subprocess.check_output([installed, "node", "info", "--json"], env=environment,
                                                 text=True, timeout=60))["did"]
        subprocess.run([str(consumer_data / "bin/elastos"), "node", "peer", "add", "--did", did, "--ticket", ticket, "--json"],
                       env=consumer_environment, stdout=subprocess.DEVNULL, check=True, timeout=60)
    except BaseException:
        cleanup_runtime(child, data)
        raise
    return child, {"holder_did": did, "consumer_holder_route": "operator peer store ticket"}


def consumer_root_block(data, cid):
    """Observe the package root in the consumer's Kubo blockstore after Get.

    Every piece read resolves its path through this root, and local reads are
    offline-only, so an absent root means every admitted byte came over Carrier.
    """
    blocks = data / "ipfs-repo/blocks"
    sharding = blocks / "SHARDING"
    # Runtime admits only Kubo's default flatfs layout: blocks/<next-to-last 2>/<base32 multihash>.data.
    if not sharding.is_file() or sharding.read_text().strip() != "/repo/flatfs/shard/v1/next-to-last/2":
        return "unobserved"
    encoded = cid[1:].upper()
    raw = base64.b32decode(encoded + "=" * (-len(encoded) % 8))
    if cid[0] != "b" or raw[:2] != b"\x01\x70":
        return "unobserved"
    key = base64.b32encode(raw[2:]).decode().rstrip("=")
    return "present" if (blocks / key[-3:-1] / f"{key}.data").exists() else "absent"


class RepairRuntime:
    """Own the installed Home and its one deliberate interrupted-Get restart."""
    def __init__(self, home, data, address, environment):
        self.home, self.data, self.address, self.environment = home, data, address, environment
        self.control = home / "repair-control"
        self.control.mkdir(mode=0o700)
        self.log = (home / "journey-runtime.private.log").open("wb")
        self.engine_fds = {}
        self.stop = threading.Event()
        self.launch_lock = threading.Lock()
        self.error = None
        try:
            self.launch()
            self.record = {"runtime_episodes": [self.identity()], "actions": []}
        except BaseException:
            if hasattr(self, "child"):
                cleanup_runtime(self.child, self.data)
            self.log.close()
            raise
        self.thread = threading.Thread(target=self.observe, daemon=True)
        self.thread.start()

    @property
    def pid(self):
        return self.child.pid

    def poll(self):
        return self.child.poll()

    def wait(self, timeout=None):
        return self.child.wait(timeout=timeout)

    def launch(self):
        with self.launch_lock:
            if self.stop.is_set():
                raise RuntimeError("repair journey owner is stopping")
            self.child = subprocess.Popen([str(self.data / "bin/elastos"), "gateway", "--addr", self.address],
                                          env=self.environment, stdout=self.log, stderr=self.log, start_new_session=True)
            return self.child

    def identity(self):
        row = linux_process_identity(self.child.pid)
        return {"pid": row["pid"], "start_ticks": row["start_ticks"], "exe_sha256": digest(Path(row["executable"]))}

    def engine_running(self):
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline and not self.stop.is_set():
            rows = owned_processes(process_rows(), self.pid, self.data)
            engines = [(pid, row) for pid, row in rows.items() if row["role"] == "llama_server"]
            if len(engines) == 1:
                pid, engine = engines[0]
                guard = rows.get(engine["ppid"])
                provider = rows.get(guard["ppid"]) if guard else None
                if not guard or guard["role"] != "guard" or not provider or provider["role"] != "model_provider" or provider["ppid"] != self.pid:
                    raise RuntimeError("repair journey requires the production guard chain")
                native, fds = [], {}
                try:
                    for member, role in ((pid, "llama_server"), (engine["ppid"], "guard"), (guard["ppid"], "model_provider")):
                        row = linux_process_identity(member)
                        if member != guard["ppid"]:
                            fds[member] = os.pidfd_open(member, 0)
                            if pidfd_exited(fds[member]) or not same_process(row, linux_process_identity(member)):
                                raise RuntimeError("repair journey native process changed during pidfd binding")
                        expected = digest(self.data / "bin/model-provider") if member != pid else engine_receipt(self.data)["executable_sha256"]
                        if digest(Path(row["executable"])) != expected or not same_process(row, linux_process_identity(member)):
                            raise RuntimeError("repair journey native artifact binding differs")
                        native.append({"role": role, "pid": member, "start_ticks": row["start_ticks"], "exe_sha256": expected})
                except BaseException:
                    for fd in fds.values():
                        os.close(fd)
                    raise
                self.engine_fds = fds
                self.record["native_processes"] = native
                return {"engine_pid": pid, "engine_start_ticks": native[0]["start_ticks"], "production_guard": True}
            time.sleep(.05)
        raise RuntimeError("repair journey engine deadline")

    def engine_stopped(self):
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline and not self.stop.is_set():
            if self.engine_fds and all(pidfd_exited(fd) for fd in self.engine_fds.values()):
                for fd in self.engine_fds.values():
                    os.close(fd)
                self.engine_fds.clear()
                return {"whole_engine_exit": True, "runtime_unchanged": self.child.poll() is None and self.identity() == self.record["runtime_episodes"][0]}
            time.sleep(.05)
        raise RuntimeError("repair journey Stop closure deadline")

    def restart_get(self):
        weights = self.data / "model-preparation/stage/weights.gguf"
        deadline = time.monotonic() + 120
        while time.monotonic() < deadline and not self.stop.is_set():
            try:
                size = weights.lstat().st_size
                if size >= 8 * 1024 ** 2:
                    break
            except FileNotFoundError:
                pass
            time.sleep(.02)
        else:
            raise RuntimeError("repair journey partial Get deadline")
        # Popen is the direct reap owner. Only this installed Runtime is killed.
        self.child.kill()
        self.child.wait(timeout=5)
        cleanup = cleanup_runtime(self.child, self.data)
        if cleanup["status"] != "passed":
            raise RuntimeError("repair journey interrupted Home cleanup failed")
        fd = os.open(weights, os.O_RDONLY | os.O_NOFOLLOW)
        with os.fdopen(fd, "rb") as source:
            metadata = os.fstat(source.fileno())
            if not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.getuid() or metadata.st_nlink != 1:
                raise RuntimeError("repair journey kept weights ownership differs")
            whole = metadata.st_size - metadata.st_size % 65536
            if not 0 < whole < 1117320736:
                raise RuntimeError("repair journey requires partial pinned Qwen weights")
            value = hashlib.sha256()
            remaining = whole
            while remaining:
                if self.stop.is_set():
                    raise RuntimeError("repair journey owner stopped during prefix binding")
                part = source.read(min(1024 ** 2, remaining))
                if not part:
                    raise RuntimeError("repair journey kept prefix changed")
                value.update(part)
                remaining -= len(part)
        self.kept = {"weights_bytes": whole, "weights_prefix_sha256": value.hexdigest(), "interrupted_process_cleanup": cleanup}
        self.launch()
        wait_healthy(self.child, self.address, "restarted Runtime", self.stop)
        self.record["runtime_episodes"].append(self.identity())
        if weights.lstat().st_size < whole:
            raise RuntimeError("repair journey restart lost its kept prefix")
        return {**self.kept, "home_restarted": True}

    def observe(self):
        previous = 0
        try:
            while not self.stop.wait(.05):
                try:
                    request = private_json(self.control / "request.json")
                except FileNotFoundError:
                    continue
                if request.get("id") == previous:
                    continue
                if set(request) != {"id", "action"} or request["id"] != previous + 1 or len(self.record["actions"]) >= 4:
                    raise RuntimeError("repair journey control binding differs")
                action = request["action"]
                if action not in ("engine_running", "engine_stopped", "restart_get") or action in self.record["actions"]:
                    raise RuntimeError("repair journey control action differs")
                response = getattr(self, action)()
                previous = request["id"]
                self.record["actions"].append(action)
                private_json(self.control / "response.json", {"id": previous, "status": "passed", **response})
        except BaseException:
            self.error = "installed_model_repair_control_failed"
            private_json(self.control / "response.json", {"id": previous + 1, "status": "failed"})

    def finish(self):
        self.stop.set()
        self.thread.join(timeout=125)
        for fd in self.engine_fds.values():
            os.close(fd)
        self.engine_fds.clear()
        with self.launch_lock:
            result = cleanup_runtime(self.child, self.data)
            self.log.close()
        if self.thread.is_alive() or self.error:
            result["status"] = "failed"
        self.record["process_cleanup"] = result
        return result

    def retry_receipt(self, ui, expected_sha):
        operation = ui["model_repair"]["retry_operation_id"]
        if not re.fullmatch("[0-9a-f]{64}", operation):
            raise RuntimeError("repair journey Retry binding differs")
        text = bounded_text(self.home / "journey-runtime.private.log", 16 * 1024 ** 2)
        text = re.sub(r"\x1b\[[0-9;]*m", "", text)
        rows = {}
        for message, fields in (("resumed", ("completed_bytes",)), ("remaining weights read", ("completed_bytes", "weights_offset"))):
            lines = [line for line in text.splitlines() if "private model preparation " + message in line and "operation_id=" + operation in line]
            if len(lines) != 1:
                raise RuntimeError("repair journey remaining fetch evidence is incomplete")
            rows[message] = {}
            for field in fields:
                value = re.search(r"\b" + field + r"=([0-9]+)\b", lines[0])
                if value is None:
                    raise RuntimeError("repair journey remaining fetch field is incomplete")
                rows[message][field] = int(value[1])
        if (rows["remaining weights read"]["weights_offset"] != self.kept["weights_bytes"]
                or rows["resumed"]["completed_bytes"] < self.kept["weights_bytes"]
                or rows["remaining weights read"]["completed_bytes"] != rows["resumed"]["completed_bytes"]):
            raise RuntimeError("repair journey Retry restarted its retained weights")
        state = json.loads(bounded_text(self.data / "model-preparation/state.json", 8 * 1024 ** 2))
        record = next(row for row in state["records"] if row["operation_id"] == operation)
        if record["state"] != "admitted" or not re.fullmatch("[0-9a-f]{64}", record["admission_id"]):
            raise RuntimeError("repair journey final admission binding differs")
        admitted = self.data / ("model-preparation/admitted-" + record["admission_id"]) / "weights.gguf"
        value = hashlib.sha256()
        with admitted.open("rb") as source:
            remaining = self.kept["weights_bytes"]
            while remaining:
                part = source.read(min(1024 ** 2, remaining))
                if not part:
                    raise RuntimeError("repair journey admitted prefix is incomplete")
                value.update(part)
                remaining -= len(part)
        if value.hexdigest() != self.kept["weights_prefix_sha256"] or digest(admitted) != expected_sha:
            raise RuntimeError("repair journey retained or final pinned weights differ")
        return {**self.kept, "operation_id": operation, "resumed_completed_bytes": rows["resumed"]["completed_bytes"],
                "first_remaining_weights_offset": rows["remaining weights read"]["weights_offset"],
                "remaining_weights_bytes": admitted.stat().st_size - self.kept["weights_bytes"],
                "admitted_weights_sha256": expected_sha, "retained_prefix_matches_admitted": True}


def run(home, data, evidence, model=True, carrier=False, memory_gib=0, repair=False):
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
    # Package, its Kubo blocks and the admitted model cache can each own a copy.
    needed = 3 * sum(path.stat().st_size for path in inputs.iterdir() if path.is_file())
    if repair:
        needed += 3 * sum(path.stat().st_size for path in (inputs.parent / "qwen-inputs").iterdir() if path.is_file())
    require_disk_space(data, needed)
    holder = holder_data = None
    package_data = data
    if carrier:
        # Only a separate holder Home's Kubo holds the package. The consumer knows the
        # holder as one Carrier peer, so its Get reads every piece over Carrier.
        holder_home = home.parent / f"{home.name}-holder"
        holder_data = package_data = fresh_fixture(home, data, holder_home, include_engine=False)
        for owner in (holder_data, data):
            with (owner / "config.toml").open("a") as config:
                config.write(f'carrier_bind_addr = "127.0.0.1:{free_port(socket.SOCK_DGRAM)}"\n')
    subprocess.run(["node", "scripts/ci-model-package.mjs", str(package_data), str(data), str(inputs), str(evidence), *(["--model-repair"] if repair else [])],
                   env=environment, check=True)
    record["fixture_package_receipt_sha256"] = digest(evidence / "package.json")
    record["fixture_components_sha256"] = digest(data / "components.json")
    cid = json.loads((evidence / "package.json").read_text())["cid"]
    require_disk_space(data)
    prior = owned_processes(process_rows(), -1, data)
    record["processes_before_launch"] = process_counts(prior)
    if prior:
        raise RuntimeError("installed journey fixture already owns running processes")
    if carrier:
        assert not (data / "ipfs-repo").exists() and not (data / "ipfs-repo").is_symlink(), "consumer Home starts without the package"
        holder, record["package_holder"] = start_holder(holder_home, holder_data, data, environment)
    number = free_port()
    address = f"localhost:{number}"
    started = time.monotonic()
    record["fixture_preparation_seconds"] = round(started - preparation_started, 2)
    # Runtime logs stay private in the isolated Home; upload receipts/screenshots only.
    finite = None
    repair_runtime = None
    log = None
    try:
        if repair:
            repair_runtime = RepairRuntime(home, data, f"127.0.0.1:{number}", environment)
            child = repair_runtime
        elif memory_gib:
            finite = FiniteRuntime(home, data, f"127.0.0.1:{number}", environment, memory_gib)
            if holder:
                finite.outside.append(holder.pid)
            child = finite.start()
        else:
            log = (home / "journey-runtime.private.log").open("wb")
            child = subprocess.Popen([str(installed), "gateway", "--addr", f"127.0.0.1:{number}"], env=environment, stdout=log, stderr=log, start_new_session=True)
    except BaseException:
        if finite:
            record["finite_memory"] = finite.finish()
            record["process_cleanup"] = {"status": record["finite_memory"]["status"]}
            record["results"]["finite_memory_boundary"] = record["finite_memory"]["status"]
            (evidence / "installed-journeys.json").write_text(json.dumps(record, indent=2) + "\n")
        if holder:
            cleanup_runtime(holder, holder_data)
        raise
    try:
        wait_healthy(child, address, "Runtime")
        command = ["node", "scripts/ci-installed-home-journey.mjs", f"http://{address}", str(evidence), str(data)]
        if finite:
            command += ["--memory-refusal" if memory_gib == 1 else "--resource-lifecycle", str(finite.control)]
        elif repair:
            command += ["--model-repair", str(repair_runtime.control)]
        if model:
            observer = TIMING["ModelTimingObserver"](child, data, started)
            try:
                with observer:
                    if finite:
                        browser_owner = subprocess.Popen(command, env=environment)
                        finite.outside.append(browser_owner.pid)
                        try:
                            deadline = time.monotonic() + 1500
                            while browser_owner.poll() is None and time.monotonic() < deadline and not finite.error:
                                time.sleep(.1)
                            if browser_owner.poll() != 0 or finite.error:
                                raise RuntimeError("finite installed UI journey failed" + failure_suffix(finite.failure))
                        finally:
                            if browser_owner.poll() is None:
                                browser_owner.terminate()
                                try:
                                    browser_owner.wait(timeout=5)
                                except subprocess.TimeoutExpired:
                                    browser_owner.kill()
                                    browser_owner.wait(timeout=5)
                            finite.outside.remove(browser_owner.pid)
                    else:
                        subprocess.run(command, env=environment, check=True)
            finally:
                record["model_timing"] = observer.receipt()
        else:
            subprocess.run([*command, "--home-only"], env=environment, check=True)
        ui = json.loads((evidence / "home-journey.json").read_text())
        record["results"].update(ui["results"])
        if repair:
            fixture = json.loads((evidence / "package.json").read_text())
            record["model_repair"] = {**ui["model_repair"], **repair_runtime.record,
                                      "retry": repair_runtime.retry_receipt(ui, fixture["models"][1]["model_sha256"])}
            record["results"]["model_remaining_fetch"] = "passed"
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
            if repair_runtime:
                record["process_cleanup"] = repair_runtime.finish()
                record.setdefault("model_repair", {}).update(repair_runtime.record)
            elif finite:
                record["finite_memory"] = finite.finish()
                try:
                    record["finite_memory"]["browser_closed"] = private_json(finite.control / "browser-stopped.json") == {"closed": True}
                except (OSError, ValueError, RuntimeError):
                    record["finite_memory"]["browser_closed"] = False
                if not record["finite_memory"]["browser_closed"]:
                    record["finite_memory"]["status"] = "failed"
                record["process_cleanup"] = {"status": record["finite_memory"]["status"],
                                             "before": {role: sum(row["role"] == role for row in finite.known.values())
                                                        for role in ("runtime", "model_provider", "guard", "llama_server", "other")}}
                if record["finite_memory"].get("cgroup_empty") and record["finite_memory"].get("whole_process_exit"):
                    record["process_cleanup"]["after"] = {role: 0 for role in ("runtime", "model_provider", "guard", "llama_server", "other")}
                record["results"]["finite_memory_boundary"] = record["finite_memory"]["status"]
            else:
                record["process_cleanup"] = cleanup_runtime(child, data, allow_group=observer_complete)
        except (OSError, ValueError, subprocess.SubprocessError):
            record["process_cleanup"] = {"status": "failed", "reason": "process census unavailable"}
        finally:
            if holder:
                try:
                    holder_cleanup = cleanup_runtime(holder, holder_data)
                except (OSError, ValueError, subprocess.SubprocessError):
                    holder_cleanup = {"status": "failed", "reason": "process census unavailable"}
            if log:
                log.close()
            if model:
                timing = record.setdefault("model_timing", {})
                timing["provider_stages"] = TIMING["stage_timings"](home / "journey-runtime.private.log")
                timing["acknowledgements"] = TIMING["acknowledgement_timings"](home / "journey-runtime.private.log")
                timing["durations"] = (TIMING["run_metrics"](timing["provider_stages"], timing["acknowledgements"])
                                       if sys.platform == "darwin" else
                                       {"status": "unavailable_with_confined_provider_stderr"})
                record["results"]["model_timing_observer"] = "passed" if timing.get("observer_complete") is True else "failed"
        if holder:
            record["process_cleanup"]["holder"] = holder_cleanup
            if holder_cleanup["status"] != "passed":
                record["process_cleanup"]["status"] = "failed"
        record["results"]["process_cleanup"] = record["process_cleanup"]["status"]
        record["disk_after"] = disk_observation(data)
        record["results"]["disk_reserve"] = ("passed" if record["disk_after"]["available_bytes"] >= DISK_RESERVE_BYTES else "failed")
        record["elapsed_seconds"] = round(time.monotonic() - started, 2)
        if (evidence / "home-journey.json").exists():
            ui = json.loads((evidence / "home-journey.json").read_text())
            record["results"].update(ui["results"])
        if carrier:
            admitted = record["results"].get("model_package_admission" if model else "engine_absent_refusal") == "passed"
            record["consumer_package_root_block"] = consumer_root_block(data, cid)
            record["results"]["carrier_get"] = ("passed" if admitted and record["consumer_package_root_block"] == "absent"
                                                else "failed")
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
    if carrier and record["results"]["carrier_get"] != "passed":
        raise RuntimeError("installed Get did not prove Carrier-only package delivery")
    if model and record["results"]["model_timing_observer"] != "passed":
        raise RuntimeError("installed timing observer did not stop")
    if memory_gib and not finite_receipt_valid(record, memory_gib):
        raise RuntimeError("installed finite-memory journey proof is incomplete")
    if repair and any(record["results"].get(name) != "passed" for name in ("model_stop_get_different", "model_interrupted_get_retry", "model_remaining_fetch")):
        raise RuntimeError("installed model repair journey proof is incomplete")
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
    if len(sys.argv) == 3 and sys.argv[1] == "--runtime-unit-helper":
        runtime_unit_helper(Path(sys.argv[2]).resolve())
        raise SystemExit(0)
    parser = argparse.ArgumentParser()
    parser.add_argument("home", type=Path)
    parser.add_argument("data", type=Path)
    parser.add_argument("evidence", type=Path)
    parser.add_argument("--repeat", type=int, choices=[3])
    parser.add_argument("--home-only", action="store_true")
    # One reply Get from a separate holder Home over Carrier; other runs use a same-Home package.
    parser.add_argument("--carrier-get", action="store_true")
    parser.add_argument("--finite-memory", type=int, choices=[1, 4])
    parser.add_argument("--model-repair", action="store_true", help="real Stop, Get different model and interrupted Get/Home restart/Retry")
    args = parser.parse_args()
    if args.model_repair and (sys.platform != "linux" or args.home_only or args.repeat or args.finite_memory):
        parser.error("--model-repair proves one production Linux Home with two signed models")
    if args.home_only and args.repeat:
        parser.error("--home-only has one engine-absent Home")
    if args.carrier_get and (args.home_only or args.repeat):
        parser.error("--carrier-get proves one reply Get")
    if args.finite_memory and (sys.platform != "linux" or args.home_only or args.repeat
                              or args.finite_memory == 1 and args.carrier_get):
        parser.error("--finite-memory proves Linux 1 GiB refusal or 4 GiB lifecycle")
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
        destination = home.parent / ("model-repair-home" if args.model_repair else "memory-refusal-home" if args.finite_memory == 1 else "reply-home")
        arguments = [destination, fresh_fixture(home, data, destination), evidence, True, args.carrier_get or args.model_repair, args.finite_memory or 0, args.model_repair]
    operation(*arguments)
