#!/usr/bin/env python3
"""Prove publisher discovery keeps gateway health when bootstrap lives on another URL."""

import importlib.util
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
import urllib.error

HELPER = Path(__file__).resolve().with_name("discover-source-bootstrap.py")
spec = importlib.util.spec_from_file_location("discover_source_bootstrap", HELPER)
bootstrap = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bootstrap)


def http_404(url):
    return urllib.error.HTTPError(url, 404, "Not found", hdrs=None, fp=io.BytesIO(b"Not found"))


class DiscoverSourceBootstrapTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="discover-source-bootstrap-")
        self.data = Path(self.temp.name)
        self.control = "http://127.0.0.1:60474"
        self.public = "http://127.0.0.1:61700"
        self.gateway_coords = self.data / "gateway-runtime-coords.json"
        self.operator_coords = self.data / "runtime-coords.json"
        self.gateway_coords.write_text(json.dumps({
            "api_url": self.control,
            "runtime_kind": "gateway",
            "attach_secret": "control-secret",
        }))
        self.operator_coords.write_text(json.dumps({
            "api_url": self.public,
            "runtime_kind": "operator",
            "attach_secret": "operator-secret",
        }))
        self.addCleanup(self.temp.cleanup)
        self.addCleanup(lambda: os.environ.pop("ELASTOS_SOURCE_PUBLISHER_URL", None))
        os.environ.pop("ELASTOS_SOURCE_PUBLISHER_URL", None)

    def opener(self, pages):
        def fetch(url, timeout):
            if url not in pages:
                raise http_404(url)
            payload = pages[url]
            if isinstance(payload, Exception):
                raise payload
            return payload
        return fetch

    def test_fresh_gateway_uses_gateway_coords_when_legacy_file_is_absent(self):
        self.operator_coords.unlink()
        missing = self.data / "runtime-coords.json"
        pages = {
            self.control + "/api/health": {"status": "ok", "version": "0.7.1-dev"},
            self.control + bootstrap.BOOTSTRAP_PATH: {
                "schema": "elastos.carrier.bootstrap/v1",
                "role": "publisher",
                "ticket": "control-ticket",
                "node_id": "control-node",
            },
        }
        self.assertEqual(
            bootstrap.select_control_coords(str(missing), self.data),
            self.gateway_coords,
        )
        result = bootstrap.discover(str(missing), self.data, self.opener(pages))
        self.assertEqual(result, {"version": "0.7.1-dev"})

    def test_explicit_publisher_url_overrides_leftover_operator_bootstrap(self):
        advertised = "http://127.0.0.1:8090"
        os.environ["ELASTOS_SOURCE_PUBLISHER_URL"] = advertised
        pages = {
            self.control + "/api/health": {"status": "ok", "version": "0.7.1-dev"},
            self.public + bootstrap.BOOTSTRAP_PATH: {
                "schema": "elastos.carrier.bootstrap/v1",
                "role": "publisher",
                "ticket": "leftover-ticket",
                "node_id": "leftover-node",
            },
            advertised + bootstrap.BOOTSTRAP_PATH: {
                "schema": "elastos.carrier.bootstrap/v1",
                "role": "publisher",
                "ticket": "advertised-ticket",
                "node_id": "advertised-node",
            },
        }
        result = bootstrap.discover(self.gateway_coords, self.data, self.opener(pages))
        self.assertEqual(result, {
            "version": "0.7.1-dev",
            "ticket": "advertised-ticket",
            "node_id": "advertised-node",
            "role": "publisher",
        })

    def test_health_survives_when_publisher_bootstrap_is_absent(self):
        pages = {
            self.control + "/api/health": {"status": "ok", "version": "0.7.1-dev"},
            self.public + bootstrap.BOOTSTRAP_PATH: {
                "schema": "elastos.carrier.bootstrap/v1",
                "role": "publisher",
                "ticket": "leftover-ticket",
                "node_id": "leftover-node",
            },
        }
        result = bootstrap.discover(self.gateway_coords, self.data, self.opener(pages))
        self.assertEqual(result, {"version": "0.7.1-dev"})
        self.assertNotIn("ticket", result)
        self.assertNotIn("node_id", result)

    def test_legacy_same_url_reads_health_and_bootstrap(self):
        self.gateway_coords.unlink()
        pages = {
            self.public + "/api/health": {"status": "ok", "version": "0.7.0-dev"},
            self.public + bootstrap.BOOTSTRAP_PATH: {
                "schema": "elastos.carrier.bootstrap/v1",
                "role": "publisher",
                "ticket": "publisher-ticket",
                "node_id": "publisher-node",
            },
        }
        result = bootstrap.discover(self.operator_coords, self.data, self.opener(pages))
        self.assertEqual(result, {
            "version": "0.7.0-dev",
            "ticket": "publisher-ticket",
            "node_id": "publisher-node",
            "role": "publisher",
        })

    def test_publisher_pair_survives_when_health_is_absent(self):
        self.gateway_coords.unlink()
        pages = {
            self.public + bootstrap.BOOTSTRAP_PATH: {
                "schema": "elastos.carrier.bootstrap/v1",
                "role": "publisher",
                "ticket": "publisher-ticket",
                "node_id": "publisher-node",
            },
        }
        result = bootstrap.discover(self.operator_coords, self.data, self.opener(pages))
        self.assertEqual(result, {
            "ticket": "publisher-ticket",
            "node_id": "publisher-node",
            "role": "publisher",
        })
        self.assertNotIn("version", result)

    def test_runtime_role_and_incomplete_pair_are_rejected(self):
        self.gateway_coords.unlink()
        pages = {
            self.public + "/api/health": {"status": "ok", "version": "0.7.1-dev"},
            self.public + bootstrap.BOOTSTRAP_PATH: {
                "schema": "elastos.carrier.bootstrap/v1",
                "role": "runtime",
                "ticket": "runtime-ticket",
                "node_id": "runtime-node",
            },
        }
        result = bootstrap.discover(self.operator_coords, self.data, self.opener(pages))
        self.assertEqual(result, {"version": "0.7.1-dev"})

        pages[self.public + bootstrap.BOOTSTRAP_PATH] = {
            "schema": "elastos.carrier.bootstrap/v1",
            "role": "publisher",
            "ticket": "publisher-ticket",
            "node_id": "",
        }
        result = bootstrap.discover(self.operator_coords, self.data, self.opener(pages))
        self.assertEqual(result, {"version": "0.7.1-dev"})


if __name__ == "__main__":
    unittest.main()
