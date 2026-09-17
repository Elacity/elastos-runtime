#!/usr/bin/env python3
"""Resolve trusted-source health version and publisher Carrier bootstrap.

Health comes from one selected control coords file. Bootstrap comes from
ELASTOS_SOURCE_PUBLISHER_URL, or from that same control URL on a legacy
operator layout. A fresh gateway has only gateway-runtime-coords.json.
"""

import json
import os
import pathlib
import sys
import urllib.request

sys.dont_write_bytecode = True

BOOTSTRAP_PATH = "/.well-known/elastos/carrier-bootstrap.json?role=publisher"


def read_coords(path):
    try:
        return json.loads(pathlib.Path(path).read_text())
    except Exception:
        return None


def read_api_url(path):
    coords = read_coords(path)
    if not isinstance(coords, dict):
        return None
    api = (coords.get("api_url") or "").rstrip("/")
    return api or None


def select_control_coords(coords_path, data_dir):
    explicit = pathlib.Path(coords_path) if coords_path else None
    if explicit and explicit.is_file():
        return explicit
    if not data_dir:
        return None
    data = pathlib.Path(data_dir)
    gateway = data / "gateway-runtime-coords.json"
    if gateway.is_file():
        return gateway
    legacy = data / "runtime-coords.json"
    if legacy.is_file():
        return legacy
    return None


def is_gateway_control(path):
    if path.name == "gateway-runtime-coords.json":
        return True
    coords = read_coords(path)
    return isinstance(coords, dict) and coords.get("runtime_kind") == "gateway"


def fetch_json(url, timeout, opener=None):
    if opener is not None:
        return opener(url, timeout)
    with urllib.request.urlopen(url, timeout=timeout) as resp:
        return json.loads(resp.read().decode())


def publisher_pair(data):
    if data.get("schema") != "elastos.carrier.bootstrap/v1":
        return None
    if data.get("role") != "publisher":
        return None
    ticket = (data.get("ticket") or "").strip()
    node_id = (data.get("node_id") or "").strip()
    if not ticket or not node_id:
        return None
    return ticket, node_id


def discover(coords_path, data_dir, opener=None):
    control = select_control_coords(coords_path, data_dir)
    if control is None:
        return {}
    control_url = read_api_url(control)
    explicit_publisher = (os.environ.get("ELASTOS_SOURCE_PUBLISHER_URL") or "").rstrip("/")
    if explicit_publisher:
        bootstrap_url = explicit_publisher
    elif is_gateway_control(control):
        bootstrap_url = None
    else:
        bootstrap_url = control_url

    version = ""
    if control_url:
        try:
            health = fetch_json(control_url + "/api/health", 2, opener)
        except Exception:
            health = None
        if isinstance(health, dict):
            version = health.get("version") or ""

    ticket = ""
    node_id = ""
    if bootstrap_url:
        try:
            bootstrap = fetch_json(bootstrap_url + BOOTSTRAP_PATH, 5, opener)
        except Exception:
            bootstrap = None
        pair = publisher_pair(bootstrap) if isinstance(bootstrap, dict) else None
        if pair:
            ticket, node_id = pair

    result = {}
    if version:
        result["version"] = version
    if ticket and node_id:
        result["ticket"] = ticket
        result["node_id"] = node_id
        result["role"] = "publisher"
    return result


def main():
    print(json.dumps(discover(
        os.environ.get("COORDS_PATH", ""),
        os.environ.get("DATA_DIR", ""),
    )))


if __name__ == "__main__":
    main()
