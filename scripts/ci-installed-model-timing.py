#!/usr/bin/env python3
"""Collect bounded model timings without publishing private Runtime logs."""
import http.client
import io
import json
import re
import socket
import subprocess
import sys
import threading
import time


STAGES = {
    "artifact_validation_started", "artifact_validation_completed",
    "guard_started", "guard_initialized", "engine_ready", "engine_timeout",
    "engine_failed", "run_started", "input_tokens_started",
    "input_tokens_completed", "generation_started", "first_delta",
    "generation_completed", "run_timeout", "run_failed",
}
STAGE_LINE = re.compile(r"\[model-provider\] local timing stage=([a-z_]+) elapsed_ms=([0-9]{1,10})\s*\Z")


def stage_timings(log):
    result = []
    with log.open(errors="replace") as lines:
        for line in lines:
            match = STAGE_LINE.fullmatch(line)
            if match and match[1] in STAGES and int(match[2]) <= 3_600_000:
                result.append({"stage": match[1], "elapsed_ms": int(match[2])})
                if len(result) == 512:
                    break
    return result


def owned_engine(rows, gateway_pid, data):
    """Observe only the installed provider's guard child in this live Home."""
    for pid, (parent, command) in rows.items():
        if not command.startswith(str(data) + "/") or "/llama-server -m " not in command:
            continue
        guard = rows.get(parent)
        provider = rows.get(guard[0]) if guard else None
        provider_path = str(data / "bin/model-provider")
        if not guard or guard[1] != provider_path + " --internal-local-llama-guard":
            continue
        if not provider or not (provider[1] == provider_path or provider[1].startswith(provider_path + " ")):
            continue
        if provider[0] != gateway_pid:
            continue
        host = re.search(r"--host\s+(/\S+\.engine\.sock)\s+--ctx-size", command)
        alias = re.search(r"--alias\s+([0-9a-f]{32})(?:\s|$)", command)
        if host and alias:
            yield pid, host[1], alias[1]


def matching_alias(path, alias):
    # This direct diagnostic cannot establish Runtime broker admission.
    deadline = time.monotonic() + .25
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as channel:
            def bound():
                left = deadline - time.monotonic()
                if left <= 0:
                    raise TimeoutError()
                channel.settimeout(left)
            bound()
            channel.connect(path)
            bound()
            channel.sendall(b"GET /v1/models HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            wire = bytearray()
            while len(wire) <= 49152:
                bound()
                chunk = channel.recv(min(8192, 49153 - len(wire)))
                if not chunk:
                    break
                wire.extend(chunk)
            if len(wire) > 49152:
                return "oversize"
            class BufferedSocket:
                def makefile(self, _):
                    return io.BytesIO(wire)
            response = http.client.HTTPResponse(BufferedSocket())
            response.begin()
            body = response.read(16385)
            if response.status != 200:
                return "unavailable"
            if len(body) > 16384:
                return "oversize"
            payload = json.loads(body)
            return ("matching_alias" if any(isinstance(row, dict) and row.get("id") == alias
                    for row in payload.get("data", [])) else "wrong_alias")
    except (socket.timeout, TimeoutError):
        return "timeout"
    except (OSError, ValueError, TypeError, AttributeError, http.client.HTTPException):
        return "unavailable"


class ModelTimingObserver:
    def __init__(self, gateway, data, started):
        self.gateway, self.data, self.started = gateway, data, started
        self.stop = threading.Event()
        self.events = []
        self.thread = None

    def __enter__(self):
        if sys.platform == "darwin":
            self.thread = threading.Thread(target=self.observe, daemon=True)
            self.thread.start()
        return self

    def observe(self):
        seen = {}
        while not self.stop.is_set() and self.gateway.poll() is None:
            if len(self.events) >= 512:
                break
            try:
                output = subprocess.check_output(["ps", "-axo", "pid=,ppid=,args="], text=True, timeout=1)
                rows = {}
                for line in output.splitlines():
                    fields = line.strip().split(None, 2)
                    if len(fields) == 3:
                        rows[int(fields[0])] = (int(fields[1]), fields[2])
                for pid, path, alias in owned_engine(rows, self.gateway.pid, self.data):
                    if pid not in seen:
                        seen[pid] = False
                        self.events.append({"stage": "engine_spawn_observed", "elapsed_seconds": round(time.monotonic() - self.started, 3)})
                    if not seen[pid]:
                        result = matching_alias(path, alias)
                        self.events.append({"stage": "direct_engine_health_probe", "result": result,
                                            "elapsed_seconds": round(time.monotonic() - self.started, 3)})
                        seen[pid] = result == "matching_alias"
            except (OSError, ValueError, subprocess.SubprocessError):
                self.events.append({"stage": "observer_unavailable", "elapsed_seconds": round(time.monotonic() - self.started, 3)})
            self.stop.wait(1)

    def __exit__(self, *_):
        self.stop.set()
        if self.thread:
            self.thread.join(timeout=2)

    def receipt(self):
        return {"direct_engine_probe_is_broker_proof": False,
                "observer_complete": self.thread is None or not self.thread.is_alive(),
                "engine_observations": list(self.events)}
