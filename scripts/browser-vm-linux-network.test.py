"""Root-free tests for the host setup, readiness and bounded lease contract."""
import contextlib
import copy
import importlib.util
import io
import json
from pathlib import Path
import stat
import shutil
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('network', Path(__file__).with_name('browser-vm-linux-network.py'))
network = importlib.util.module_from_spec(spec)
spec.loader.exec_module(network)
trusted_json = network.trusted_json


class NetworkTest(unittest.TestCase):
    def setUp(self):
        self.stack = contextlib.ExitStack()
        self.addCleanup(self.stack.close)
        root = Path(self.stack.enter_context(tempfile.TemporaryDirectory(prefix='browser-network-')))
        for name in ['RUN', 'SYSFS', 'IPV6']:
            directory = root / name
            directory.mkdir()
            self.stack.enter_context(patch.object(network, name, directory))
        self.uid = 1001
        self.pool = network.networks(self.uid)
        self.receipt = dict(uid=self.uid, boot_id='fixture-boot', networks=self.pool,
                            policy=network.policy(self.pool), ifindices=list(range(10, 14)))
        self.links = {}
        for n, index in zip(self.pool, self.receipt['ifindices']):
            device = network.SYSFS / n['tapName']
            device.mkdir()
            for name, value in dict(owner=self.uid, tun_flags='0x1002', carrier=0, ifindex=index).items():
                (device / name).write_text(str(value))
            ipv6 = network.IPV6 / n['tapName']
            ipv6.mkdir()
            (ipv6 / 'disable_ipv6').write_text('1')
            (network.RUN / f"{n['tapName']}.lock").touch()
            self.links[n['tapName']] = dict(ifindex=index, flags=['UP'],
                addr_info=[dict(family='inet', local=n['hostIp'], prefixlen=30)])
        self.calls = []
        self.stack.enter_context(patch.object(network, 'run', side_effect=self.run_command))
        self.stack.enter_context(patch.object(network, 'trusted_json', side_effect=lambda _: self.receipt))
        self.stack.enter_context(patch.object(network, 'boot_id', return_value='fixture-boot'))
        real_stat = network.os.stat
        self.stack.enter_context(patch.object(network.os, 'stat', side_effect=lambda p, **kw:
            SimpleNamespace(st_mode=stat.S_IFCHR) if str(p) in ['/dev/kvm', '/dev/net/tun'] else real_stat(p, **kw)))
        self.access = self.stack.enter_context(patch.object(network.os, 'access', return_value=True))

    def run_command(self, *args, **kwargs):
        self.calls.append(args)
        if args[:3] == ('ip', '-j', 'addr'):
            return SimpleNamespace(returncode=0, stdout=json.dumps([self.links[args[-1]]]))
        if args[:3] == ('ip', 'tuntap', 'show'):
            return SimpleNamespace(returncode=0, stdout=f'{args[-1]}: tap persist user {self.uid}')
        if args[:3] == ('ip', '-j', 'route'):
            return SimpleNamespace(returncode=0, stdout='[]')
        return SimpleNamespace(returncode=1 if '-C' in args or '-S' in args else 0, stdout='')

    def test_pool_is_bounded_and_deterministic(self):
        self.assertEqual(network.networks(self.uid), self.pool)
        self.assertEqual(len(self.pool), 4)
        self.assertEqual(len({n['hostIp'] for n in self.pool}), 4)
        self.assertEqual(len({n['tapName'] for n in self.pool}), 4)
        self.assertTrue(all(len(n['tapName']) <= 15 for n in network.networks(4294967294)))

    def test_check_is_read_only(self):
        self.assertEqual(network.check(self.uid), self.pool)
        self.assertTrue(all(c[:3] in [('ip', '-j', 'addr'), ('ip', 'tuntap', 'show')] for c in self.calls))

    def test_missing_or_wrong_setup_is_refused(self):
        original = copy.deepcopy(self.receipt)
        for field, value in [('uid', 1002), ('boot_id', 'old-boot'), ('networks', []), ('policy', {}), ('ifindices', [])]:
            with self.subTest(field=field):
                self.receipt = {**copy.deepcopy(original), field: value}
                with self.assertRaises(RuntimeError):
                    network.check(self.uid)
        self.receipt = original
        first = self.pool[0]
        for field, value in [('ifindex', 100), ('flags', []), ('master', 'br0'), ('addr_info', [])]:
            with self.subTest(field=field):
                link = self.links[first['tapName']]
                before = copy.deepcopy(link)
                link[field] = value
                with self.assertRaises(RuntimeError):
                    network.check(self.uid)
                self.links[first['tapName']] = before
        for field, value in [('owner', '1002'), ('tun_flags', '0x1001')]:
            file = network.SYSFS / first['tapName'] / field
            previous = file.read_text()
            file.write_text(value)
            with self.assertRaises(RuntimeError):
                network.check(self.uid)
            file.write_text(previous)
        self.access.return_value = False
        with self.assertRaisesRegex(RuntimeError, '/dev/kvm'):
            network.check(self.uid)
        self.access.return_value = True
        (network.SYSFS / first['tapName'] / 'owner').unlink()
        with self.assertRaises(FileNotFoundError):
            network.check(self.uid)

    def test_refusal_names_one_root_command(self):
        user = SimpleNamespace(pw_uid=self.uid, pw_name='elastos-agent')
        with patch.object(network.sys, 'argv', ['network', 'check']), patch.object(network.pwd, 'getpwuid', return_value=user), \
                patch.object(network.os, 'getuid', return_value=self.uid), \
                patch.object(network, 'check', side_effect=FileNotFoundError('missing TAP')):
            with self.assertRaisesRegex(RuntimeError, r'As root, run: python3 .*browser-vm-linux-network.py setup --user elastos-agent'):
                network.main()

    def test_lease_skips_locked_and_orphaned_taps_then_releases(self):
        with open(network.RUN / f"{self.pool[0]['tapName']}.lock", 'r+') as held:
            network.fcntl.flock(held, network.fcntl.LOCK_EX | network.fcntl.LOCK_NB)
            (network.SYSFS / self.pool[1]['tapName'] / 'carrier').write_text('1')
            output = io.StringIO()
            with patch.object(network.sys, 'stdout', output), patch.object(network.sys, 'stdin', SimpleNamespace(buffer=io.BytesIO())):
                network.lease(self.uid)
            self.assertEqual(json.loads(output.getvalue()), self.pool[2])
            with open(network.RUN / f"{self.pool[2]['tapName']}.lock", 'r+') as released:
                network.fcntl.flock(released, network.fcntl.LOCK_EX | network.fcntl.LOCK_NB)
        for n in self.pool:
            (network.SYSFS / n['tapName'] / 'carrier').write_text('1')
        with self.assertRaisesRegex(network.SlotsBusy, 'Close a Browser session'):
            network.lease(self.uid)
        with self.assertRaisesRegex(RuntimeError, 'Close Browser sessions'):
            with network.locks(self.uid, self.pool):
                self.fail('active VM must block root changes')

    def test_firewall_order_scope_and_removal(self):
        n = self.pool[0]
        network.firewall(n)
        for tool in ['iptables', 'ip6tables']:
            calls = [c for c in self.calls if c[0] == tool]
            rules = [list(c[4:]) for c in calls if c[2] == '-A']
            expected = network.input_rules(n) if tool == 'iptables' else [['-j', 'DROP']]
            self.assertEqual(rules, expected)
            self.assertIn((tool, '-w', '-I', 'FORWARD', '-i', n['tapName'], '-j', 'DROP'), calls)
            self.assertEqual(calls[-1], (tool, '-w', '-I', 'INPUT', '-i', n['tapName'], '-j', network.chain(n)))
        rules = network.input_rules(n)
        self.assertEqual(rules[-1], ['-j', 'DROP'])
        for rule in rules[:-1]:
            self.assertEqual(rule[:4], ['-s', n['guestIp'], '-d', n['hostIp']])
        self.calls.clear()
        with patch.object(network, 'run', side_effect=lambda *a, **kw: (self.calls.append(a) or SimpleNamespace(returncode=1 if '-C' in a else 0))):
            network.firewall(n, remove=True)
        self.assertTrue(all('-A' not in c and '-I' not in c for c in self.calls))
        self.assertEqual(len([c for c in self.calls if '-X' in c]), 2)

    def test_restore_constructs_user_owned_taps_and_receipt(self):
        user = SimpleNamespace(pw_uid=self.uid, pw_gid=1001)
        with patch.object(network, 'write_json') as receipt, patch.object(network.os, 'chown'), patch.object(network, 'trusted_path'):
            network.restore(user)
        self.assertIn(('setfacl', '-m', 'u:1001:rw', '/dev/kvm'), self.calls)
        for n in self.pool:
            self.assertIn(('ip', 'addr', 'add', f"{n['hostIp']}/30", 'dev', n['tapName']), self.calls)
            self.assertIn(('ip', 'link', 'set', n['tapName'], 'up'), self.calls)
        self.assertEqual(receipt.call_args.args[1], self.receipt)
        # Cover new-device command construction without touching host sysfs.
        for n in self.pool:
            shutil.rmtree(network.SYSFS / n['tapName'])
        def fake_create(*args, **kwargs):
            result = self.run_command(*args, **kwargs)
            if args[:3] == ('ip', 'tuntap', 'add'):
                device = network.SYSFS / args[4]
                device.mkdir()
                (device / 'ifindex').write_text('20')
            return result
        with patch.object(network, 'run', side_effect=fake_create), patch.object(network, 'write_json'), patch.object(network.os, 'chown'), patch.object(network, 'trusted_path'):
            network.restore(user)
        for n in self.pool:
            self.assertIn(('ip', 'tuntap', 'add', 'dev', n['tapName'], 'mode', 'tap', 'user', '1001'), self.calls)

    def test_receipt_requires_root_ownership_and_protected_paths(self):
        for uid, mode in [(0, stat.S_IFREG | 0o644), (1001, stat.S_IFREG | 0o644),
                          (0, stat.S_IFREG | 0o664), (0, stat.S_IFLNK | 0o777)]:
            file = SimpleNamespace(parents=[], lstat=lambda: SimpleNamespace(st_uid=uid, st_mode=mode), read_text=lambda: '{}')
            if uid == 0 and mode == stat.S_IFREG | 0o644:
                self.assertEqual(trusted_json(file), {})
            else:
                with self.assertRaisesRegex(RuntimeError, 'untrusted setup path'):
                    trusted_json(file)

    def test_setup_requires_confirmation_before_mutation(self):
        user = SimpleNamespace(pw_uid=self.uid, pw_name='elastos-agent')
        with patch.object(network.sys, 'argv', ['network', 'setup', '--user', 'elastos-agent']), \
                patch.object(network.pwd, 'getpwnam', return_value=user), patch.object(network.os, 'getuid', return_value=0), \
                patch.object(network.sys, 'platform', 'linux'), \
                patch('builtins.input', return_value='n'), patch.object(network, 'configure') as configure, \
                contextlib.redirect_stdout(io.StringIO()) as output:
            network.main()
            configure.assert_not_called()
            self.assertIn('KVM user ACL', output.getvalue())
            self.assertIn(self.pool[0]['tapName'], output.getvalue())

    def test_setup_and_removal_manage_only_the_account_state(self):
        root = network.RUN.parent
        unit_dir, udev_dir = root / 'systemd', root / 'udev'
        unit_dir.mkdir()
        udev_dir.mkdir()
        state_dir, helper = root / 'state', root / 'lib/network.py'
        user = SimpleNamespace(pw_uid=self.uid, pw_gid=self.uid, pw_name='elastos-agent')
        def local_path(value):
            if str(value) == '/etc/systemd/system':
                return unit_dir
            if str(value).startswith('/etc/udev/rules.d/'):
                return udev_dir / Path(value).name
            return Path(value)
        with patch.object(network, 'STATE', state_dir), patch.object(network, 'HELPER', helper), \
                patch.object(network, 'Path', side_effect=local_path), patch.object(network, 'trusted_path'), \
                patch.object(network.shutil, 'which', side_effect=lambda command: f'/usr/bin/{command}'), \
                patch.object(network, 'restore') as restore, \
                patch.object(network, 'trusted_json', side_effect=lambda file: json.loads(file.read_text())):
            network.configure(user)
            restore.assert_called_once_with(user)
            unit = unit_dir / 'elastos-browser-network-1001.service'
            self.assertIn('restore --user elastos-agent', unit.read_text())
            self.assertIn('After=network.target nftables.service', unit.read_text())
            self.assertIn('-m u:1001:rw /dev/kvm', next(udev_dir.iterdir()).read_text())
            self.assertEqual(json.loads((state_dir / '1001.json').read_text()), {'previous_kvm_acl': None})
            network.configure(user, remove=True)
            self.assertIn(('setfacl', '-x', 'u:1001', '/dev/kvm'), self.calls)
            self.assertFalse(unit.exists())
            self.assertEqual(list(udev_dir.iterdir()), [])
            self.assertEqual(list(state_dir.iterdir()), [])
            self.assertFalse(helper.exists())
            for n in self.pool:
                self.assertIn(('ip', 'tuntap', 'del', 'dev', n['tapName'], 'mode', 'tap'), self.calls)
                self.assertFalse((network.RUN / f"{n['tapName']}.lock").exists())


if __name__ == '__main__':
    unittest.main()
