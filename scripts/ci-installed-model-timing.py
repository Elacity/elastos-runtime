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
    "stream_completed", "generation_completed", "terminal_applied", "run_timeout", "run_failed",
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


ACK_LINE = re.compile(r"\[model-provider\] local acknowledgement kind=(delta|terminal) outcome=(applied|rejected|timeout|control_lost) elapsed_ms=([0-9]{1,10}) duration_ms=([0-9]{1,10})\s*\Z")


def acknowledgement_timings(log):
    result = []
    with log.open(errors="replace") as lines:
        for line in lines:
            match = ACK_LINE.fullmatch(line)
            if match and 0 <= int(match[4]) <= int(match[3]) <= 3_600_000:
                result.append({"kind": match[1], "outcome": match[2],
                               "elapsed_ms": int(match[3]), "duration_ms": int(match[4])})
                if len(result) == 512:
                    break
    return result


def run_metrics(stages, acknowledgements):
    """Worker clock starts before engine validation; endpoint clock is separate.

    Generation excludes delta Applied wait, which includes coordinator queue,
    reconciliation and durable storage. It still includes HTTP/stream handling.
    Stream completion is provider receipt of the backend terminal marker.
    """
    names = ["run_started", "generation_started", "first_delta", "stream_completed",
             "generation_completed", "terminal_applied"]
    values = {}
    for name in names:
        matches = [row["elapsed_ms"] for row in stages if row["stage"] == name]
        if len(matches) != 1:
            return {"status": "incomplete"}
        values[name] = matches[0]
    sequence = [values[name] for name in names]
    delta = [row for row in acknowledgements if row["kind"] == "delta"]
    terminal = [row for row in acknowledgements if row["kind"] == "terminal"]
    if (sequence != sorted(sequence) or sequence[-1] > 120_000
            or any(row["stage"] in {"run_failed", "run_timeout"} for row in stages)
            or not delta or len(terminal) != 1
            or any(row["outcome"] != "applied" for row in acknowledgements)
            or terminal[0]["elapsed_ms"] > values["terminal_applied"]
            or terminal[0]["elapsed_ms"] - terminal[0]["duration_ms"] < values["generation_completed"]):
        return {"status": "incomplete"}
    previous_end = values["first_delta"]
    for row in delta:
        end = row["elapsed_ms"]
        start = end - row["duration_ms"]
        if (start < previous_end or end > values["generation_completed"]
                or start < values["stream_completed"] < end):
            return {"status": "incomplete"}
        previous_end = end
    if terminal[0]["elapsed_ms"] - terminal[0]["duration_ms"] < previous_end:
        return {"status": "incomplete"}
    engine_ready = [row["elapsed_ms"] for row in stages if row["stage"] == "engine_ready"]
    if len(engine_ready) != 1:
        return {"status": "incomplete"}
    generation_wall = values["stream_completed"] - values["generation_started"]
    during_generation = sum(row["duration_ms"] for row in delta
                            if row["elapsed_ms"] <= values["stream_completed"])
    if during_generation > generation_wall:
        return {"status": "incomplete"}
    return {"status": "complete", "clock_origin": "local_text_worker_start",
            "generation_endpoint": "provider_received_stream_terminal_marker",
            "acknowledgement_owner": "model_provider_coordinator_durable_apply",
            "engine_clock_origin": "local_llama_endpoint_start",
            "engine_ready_ms": engine_ready[0],
            "generation_wall_ms": generation_wall,
            "generation_excluding_acknowledgement_ms": generation_wall - during_generation,
            "delta_acknowledgement_count": len(delta),
            "delta_acknowledgement_ms": sum(row["duration_ms"] for row in delta),
            "delta_acknowledgement_max_ms": max(row["duration_ms"] for row in delta),
            "acknowledgement_total_ms": sum(row["duration_ms"] for row in acknowledgements),
            "terminal_acknowledgement_count": 1,
            "terminal_acknowledgement_ms": terminal[0]["duration_ms"],
            "terminal_applied_ms": values["terminal_applied"]}


def timing_spread(records, expected_runs):
    """Only complete, same-candidate installed passes form a timing series."""
    metrics = [row.get("model_timing", {}).get("durations", {}) for row in records]
    identities = {"candidate": 40, "source_tree": 40,
                  "installed_runtime_sha256": 64, "installed_model_provider_sha256": 64}
    if (len(records) != expected_runs or not records
            or any(not isinstance(row.get(field), str)
                   or re.fullmatch(r"[0-9a-f]{%d}" % length, row[field]) is None
                   for row in records for field, length in identities.items())
            or len({(row.get("candidate"), row.get("source_tree"),
                     row.get("installed_runtime_sha256"), row.get("installed_model_provider_sha256"))
                    for row in records}) != 1
            or any(row.get("results", {}).get("installed_runtime_reply") != "passed" for row in records)
            or any(row.get("status") != "complete" for row in metrics)):
        return {"status": "incomplete", "expected_runs": expected_runs, "recorded_runs": len(records)}
    fields = ["engine_ready_ms", "generation_wall_ms", "generation_excluding_acknowledgement_ms",
              "acknowledgement_total_ms", "delta_acknowledgement_ms", "terminal_acknowledgement_ms", "terminal_applied_ms"]
    spread = {}
    for field in fields:
        values = [row.get(field) for row in metrics]
        if any(type(value) is not int or not 0 <= value <= 3_600_000 for value in values):
            return {"status": "incomplete", "expected_runs": expected_runs, "recorded_runs": len(records)}
        spread[field] = {"values": values, "min": min(values), "max": max(values),
                         "spread": max(values) - min(values)}
    return {"status": "complete", "expected_runs": expected_runs, "recorded_runs": len(records),
            "durations_ms": spread}


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
        # Only the journey owner reaps Runtime. This observer reads process and
        # endpoint state, so shutdown cannot race with a second poll() owner.
        while not self.stop.is_set():
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
                    if self.stop.is_set():
                        break
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
