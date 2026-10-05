#!/usr/bin/env python3
"""Tiny model-cache fixtures exercise transfers without network downloads."""
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import os
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

sys.dont_write_bytecode = True
root = Path(__file__).resolve().parents[2]


def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


preseed = module('preseed', Path(__file__).with_name('canary-model-preseed.py'))
upstream = module('upstream', root / 'scripts/release-upstream-input.py')


class PreseedTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.cache = Path(self.directory.name).resolve() / 'cache'
        self.payloads = {}
        recipes = []
        for index, name in enumerate(sorted(preseed.MODELS)):
            payload = ('GGUF' + str(index)).encode()
            url = 'https://huggingface.co/fixture/resolve/' + str(index) + '/model.gguf'
            self.payloads[url] = payload
            recipes.append({'component': name, 'platform': '*', 'format': 'raw', 'source': {
                'url': url, 'checksum': 'sha256:' + hashlib.sha256(payload).hexdigest(), 'max_bytes': 16}})
        self.recipes = {'recipes': recipes}
        self.fetch = Mock(side_effect=lambda source: io.BytesIO(self.payloads[source['url']]))
        self.disk = patch.object(upstream.shutil, 'disk_usage', return_value=SimpleNamespace(total=1000, free=500))
        self.disk.start()
        self.addCleanup(self.disk.stop)

    def seed(self):
        preseed.seed(self.recipes, self.cache, upstream, 64, self.fetch)

    def test_promotes_exact_hash_names_single_link_and_revalidates_hits(self):
        self.seed()
        expected = {'sha256-' + recipe['source']['checksum'].split(':')[1] for recipe in self.recipes['recipes']}
        self.assertEqual({path.name for path in self.cache.iterdir()}, expected)
        for recipe in self.recipes['recipes']:
            path = upstream.cached_input(recipe['source'], self.cache)
            self.assertEqual(path.read_bytes(), self.payloads[recipe['source']['url']])
            self.assertEqual(path.stat().st_nlink, 1)
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
        self.fetch.reset_mock()
        self.seed()
        self.fetch.assert_not_called()
        next(self.cache.iterdir()).write_bytes(b'wrong')
        with self.assertRaisesRegex(ValueError, 'cached upstream input failed'):
            self.seed()
        self.fetch.assert_not_called()

    def test_bad_hash_zero_or_oversize_cleans_partial_and_keeps_cache_unpromoted(self):
        first = self.recipes['recipes'][0]['source']
        for payload in (b'wrong', b'', b'x' * 17):
            with self.subTest(payload=payload):
                self.payloads[first['url']] = payload
                with self.assertRaises(ValueError):
                    self.seed()
                self.assertEqual(list(self.cache.iterdir()), [])

    def test_transfer_failure_cleans_partial(self):
        self.fetch.side_effect = OSError('fixture transfer failure')
        with self.assertRaises(OSError):
            self.seed()
        self.assertEqual(list(self.cache.iterdir()), [])

    def test_initial_reserve_refuses_before_transfer(self):
        upstream.shutil.disk_usage.return_value = SimpleNamespace(total=1000, free=213)
        with self.assertRaisesRegex(ValueError, '15%'):
            self.seed()
        self.fetch.assert_not_called()

    def test_actual_block_reserve_refuses_before_write_and_cleans_partial(self):
        upstream.shutil.disk_usage.side_effect = [SimpleNamespace(total=1000, free=214),
            SimpleNamespace(total=1000, free=214), SimpleNamespace(total=1000, free=213)]
        with self.assertRaisesRegex(ValueError, '15%'):
            self.seed()
        self.fetch.assert_called_once()
        self.assertEqual(list(self.cache.iterdir()), [])

    def test_downloads_consume_the_existing_budget_once(self):
        with patch.object(upstream, 'disk_gate', wraps=upstream.disk_gate) as gate:
            self.seed()
        self.assertEqual([call.args[1] for call in gate.call_args_list], [64, 64, 64, 59, 59, 54, 54, 49, 49])

    def test_symlink_and_shared_link_cache_hits_refuse_without_fetch(self):
        self.cache.mkdir(mode=0o700)
        recipe = self.recipes['recipes'][0]['source']
        name = self.cache / ('sha256-' + recipe['checksum'].split(':')[1])
        target = self.cache.parent / 'other'
        target.write_bytes(self.payloads[recipe['url']])
        name.symlink_to(target)
        with self.assertRaises(ValueError):
            self.seed()
        name.unlink()
        os.link(target, name)
        with self.assertRaises(ValueError):
            self.seed()
        self.fetch.assert_not_called()

    def test_selects_only_four_recorded_models_and_requires_complete_unique_recipes(self):
        self.recipes['recipes'].append({'component': 'unrelated'})
        self.seed()
        self.assertEqual(self.fetch.call_count, 4)
        self.recipes['recipes'][0] = self.recipes['recipes'][1]
        with self.assertRaisesRegex(ValueError, 'four admitted'):
            self.seed()

    def test_curl_has_fixed_https_limits_suppresses_config_and_cleans_child(self):
        source = self.recipes['recipes'][0]['source']
        child = SimpleNamespace(stdout=io.BytesIO(b'fixture'), wait=Mock(return_value=0),
                                poll=Mock(return_value=0), kill=Mock())
        with patch.object(preseed.subprocess, 'Popen', return_value=child) as process:
            with preseed.curl_stream(source) as stream:
                self.assertEqual(stream.read(), b'fixture')
        argv = process.call_args.args[0]
        self.assertEqual(argv[:2], ['/usr/bin/curl', '--disable'])
        for flag, value in (('--proto', '=https'), ('--proto-redir', '=https'), ('--max-redirs', '5'),
                            ('--connect-timeout', '30'), ('--max-time', '900'), ('--max-filesize', '16'),
                            ('--url', source['url'])):
            self.assertEqual(argv[argv.index(flag) + 1], value)
        child.wait.assert_called_once_with(timeout=5)
        self.assertTrue(child.stdout.closed)
        child.kill.assert_not_called()

    def test_interrupted_curl_is_killed_and_reaped(self):
        child = SimpleNamespace(stdout=io.BytesIO(b'fixture'), wait=Mock(return_value=0),
                                poll=Mock(return_value=None), kill=Mock())
        with patch.object(preseed.subprocess, 'Popen', return_value=child), self.assertRaises(KeyboardInterrupt):
            with preseed.curl_stream(self.recipes['recipes'][0]['source']):
                raise KeyboardInterrupt()
        child.kill.assert_called_once()
        child.wait.assert_called_once_with(timeout=5)

    def test_nonzero_curl_exit_refuses_even_complete_pinned_bytes(self):
        payload = self.payloads[self.recipes['recipes'][0]['source']['url']]
        child = SimpleNamespace(stdout=io.BytesIO(payload), wait=Mock(return_value=22),
                                poll=Mock(return_value=22), kill=Mock())
        with patch.object(preseed.subprocess, 'Popen', return_value=child), self.assertRaisesRegex(ValueError, 'transfer failed'):
            preseed.seed(self.recipes, self.cache, upstream, 64, preseed.curl_stream)
        self.assertEqual(list(self.cache.iterdir()), [])
        child.kill.assert_not_called()

    def test_public_entrypoint_refuses_local_execution(self):
        with patch.object(preseed.subprocess, 'check_output') as command, self.assertRaises(ValueError):
            preseed.main()
        command.assert_not_called()

    def test_admitted_source_recipes_supply_all_four_exact_inputs(self):
        recipe_bytes = preseed.subprocess.check_output([
            'git', '-C', str(root), 'show', '1d39373898e6bd286071336e0c829d45e14cb831:scripts/release-upstream-recipes.json'])
        admitted = json.loads(recipe_bytes)
        current = (root / 'scripts/release-upstream-recipes.json').read_bytes()
        self.assertEqual(current, recipe_bytes)
        selected = [recipe for recipe in admitted['recipes'] if recipe['component'] in preseed.MODELS]
        self.assertEqual({recipe['component'] for recipe in selected}, preseed.MODELS)
        self.assertTrue(all(recipe['source']['max_bytes'] == 12 * 1024**3 for recipe in selected))


if __name__ == '__main__':
    unittest.main()
