"""Root-free tests for the host setup, readiness and bounded lease contract."""
import contextlib
import copy
import errno
import importlib.util
import io
import json
from pathlib import Path
import stat
import shutil
import shlex
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
                            policy=network.policy(self.pool), transport='kernel-vsock',
                            ifindices=list(range(10, 14)))
        self.links = {}
        for n, index in zip(self.pool, self.receipt['ifindices']):
            device = network.SYSFS / n['tapName']
            device.mkdir()
            for name, value in dict(owner=self.uid, tun_flags='0x1002', flags='0x1', carrier=0, ifindex=index).items():
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
            SimpleNamespace(st_mode=stat.S_IFCHR) if str(p) in ['/dev/kvm', '/dev/vhost-vsock', '/dev/net/tun'] else real_stat(p, **kw)))
        self.access = self.stack.enter_context(patch.object(network.os, 'access', return_value=True))
        real_exists = Path.exists
        self.stack.enter_context(patch.object(Path, 'exists', lambda p:
            True if str(p) == '/dev/vhost-vsock' else real_exists(p)))

    def run_command(self, *args, **kwargs):
        self.calls.append(args)
        if args == ('modprobe', 'vhost_vsock') and getattr(self, 'missing_vsock_module', False):
            raise RuntimeError('modprobe: module vhost_vsock not found')
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
        self.assertTrue(all(c[:3] in [('ip', '-j', 'addr'), ('ip', 'tuntap', 'show'), ('systemctl', 'is-active', '--quiet')] for c in self.calls))

    def test_readiness_refuses_a_stopped_network_service(self):
        command = self.run_command
        def stopped(*args, **kwargs):
            if args[0] == 'systemctl':
                raise RuntimeError('network service is inactive')
            return command(*args, **kwargs)
        with patch.object(network, 'run', side_effect=stopped):
            with self.assertRaisesRegex(RuntimeError, 'inactive'):
                network.check(self.uid)

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
            with self.assertRaisesRegex(RuntimeError, r'As root, run:\n/usr/bin/python3 -I /usr/local/lib/elastos/browser-vm-linux-network.py setup --user elastos-agent'):
                network.main()

    def test_down_tap_allows_repair_locks_without_reading_carrier(self):
        device = network.SYSFS / self.pool[0]['tapName']
        (device / 'flags').write_text('0x0')
        read_text = Path.read_text
        def read(file, *args, **kwargs):
            if file == device / 'carrier':
                raise OSError(errno.EINVAL, 'down TAP has no carrier state')
            return read_text(file, *args, **kwargs)
        with patch.object(Path, 'read_text', read):
            with network.locks(self.uid, self.pool):
                pass
            user = SimpleNamespace(pw_uid=self.uid, pw_gid=self.uid)
            with patch.object(network, 'trusted_path'), patch.object(network.os, 'chown'), patch.object(network, 'write_json'):
                network.restore(user)
        # The lock still blocks repair, even with the link down.
        with open(network.RUN / f"{self.pool[0]['tapName']}.lock", 'r+') as held:
            network.fcntl.flock(held, network.fcntl.LOCK_EX | network.fcntl.LOCK_NB)
            with self.assertRaises(BlockingIOError):
                with network.locks(self.uid, self.pool):
                    self.fail('a down link cannot bypass an active lease')

    def test_carrier_down_race_allows_repair_but_other_errors_are_visible(self):
        carrier = network.SYSFS / self.pool[0]['tapName'] / 'carrier'
        read_text = Path.read_text
        for code in [errno.EINVAL, errno.EACCES]:
            def read(file, *args, **kwargs):
                if file == carrier:
                    raise OSError(code, 'carrier read failed')
                return read_text(file, *args, **kwargs)
            with patch.object(Path, 'read_text', read):
                if code == errno.EINVAL:
                    self.assertFalse(network.active_carrier(self.pool[0]['tapName']))
                else:
                    with self.assertRaises(PermissionError):
                        network.active_carrier(self.pool[0]['tapName'])

    def test_failed_service_restore_revokes_readiness_and_cuts_off_guests(self):
        receipt = network.RUN / f'{self.uid}.json'
        receipt.write_text(json.dumps(self.receipt))
        user = SimpleNamespace(pw_uid=self.uid, pw_name='elastos-agent')
        with patch.object(network.sys, 'argv', ['network', 'restore', '--user', user.pw_name]), \
                patch.object(network.pwd, 'getpwnam', return_value=user), patch.object(network.os, 'getuid', return_value=0), \
                patch.object(network.sys, 'platform', 'linux'), patch.object(network, 'trusted_path'), \
                patch.object(network, 'restore', side_effect=RuntimeError('iptables failed')):
            with self.assertRaisesRegex(RuntimeError, 'iptables failed'):
                network.main()
        self.assertFalse(receipt.exists())
        for n in self.pool:
            self.assertIn(('ip', 'link', 'set', n['tapName'], 'down'), self.calls)

    def test_repair_uses_only_a_trusted_root_helper(self):
        user = SimpleNamespace(pw_name='elastos-agent', pw_uid=self.uid)
        with patch.object(network, 'trusted_path', side_effect=RuntimeError('user-writable helper')), \
                patch.object(network.sys, 'argv', ['network', 'check']), \
                patch.object(network.pwd, 'getpwuid', return_value=user), \
                patch.object(network.os, 'getuid', return_value=self.uid), \
                patch.object(network, 'check', side_effect=RuntimeError('fixture refusal')):
            with self.assertRaises(RuntimeError) as refusal:
                network.main()
        message = str(refusal.exception)
        self.assertIn('Install the reviewed root-owned helper', message)
        self.assertIn(f'/usr/bin/python3 -I {network.HELPER} setup --user elastos-agent', message)
        self.assertNotIn(str(Path(network.__file__).parent), message)

    def test_repair_command_is_pasteable_with_an_isolated_absolute_interpreter(self):
        user = SimpleNamespace(pw_name='elastos-agent', pw_uid=self.uid)
        data = network.RUN.parent / "Home's data"
        with patch.object(network, 'STATE', network.RUN.parent / 'absent-state'):
            command = network.repair_command(user, data)
        self.assertEqual(shlex.split(command), ['/usr/bin/python3', '-I', str(network.HELPER),
            'setup', '--user', user.pw_name, '--data-dir', str(data)])

    def test_unusable_saved_kernel_transport_names_removal_before_automatic_setup(self):
        user = SimpleNamespace(pw_name='elastos-agent', pw_uid=self.uid)
        state = network.RUN.parent / 'state'
        state.mkdir()
        (state / f'{self.uid}.json').write_text('{}')
        data = str(network.RUN.parent / "Home's saved data")
        self.receipt['data_dir'] = data
        real_stat = network.os.stat
        for failure in ('missing', 'regular file', 'inaccessible'):
            def device_stat(path, **kwargs):
                if str(path) == '/dev/vhost-vsock':
                    if failure == 'missing':
                        raise FileNotFoundError(path)
                    return SimpleNamespace(st_mode=stat.S_IFREG if failure == 'regular file' else stat.S_IFCHR)
                return real_stat(path, **kwargs)
            with self.subTest(failure=failure), patch.object(network, 'STATE', state), \
                    patch.object(network.os, 'stat', side_effect=device_stat), \
                    patch.object(network.os, 'access', return_value=failure != 'inaccessible'), \
                    patch.object(network.sys, 'argv', ['network', 'check', '--data-dir', '/caller/data']), \
                    patch.object(network.pwd, 'getpwuid', return_value=user), \
                    patch.object(network.os, 'getuid', return_value=self.uid), \
                    patch.object(network, 'check', side_effect=RuntimeError('kernel device unusable')):
                with self.assertRaises(RuntimeError) as refusal:
                    network.main()
                commands = [shlex.split(line) for line in str(refusal.exception).splitlines()
                            if line.startswith('/usr/bin/python3 -I ')]
                self.assertEqual(commands, [
                    ['/usr/bin/python3', '-I', str(network.HELPER), action,
                     '--user', user.pw_name, '--data-dir', data] for action in ('remove', 'setup')])

    def test_userspace_repair_and_removal_use_the_saved_transport(self):
        self.receipt['transport'] = 'userspace-vsock'
        user = SimpleNamespace(pw_uid=self.uid, pw_name='elastos-agent')
        state = network.RUN.parent / 'state'
        state.mkdir()
        (state / f'{self.uid}.json').write_text('{}')
        with patch.object(network, 'STATE', state), patch.object(network, 'trusted_path'):
            self.assertIn('--transport userspace-vsock', network.repair_command(user))
        with patch.object(network.sys, 'argv', ['network', 'remove', '--user', user.pw_name]), \
                patch.object(network.pwd, 'getpwnam', return_value=user), patch.object(network.os, 'getuid', return_value=0), \
                patch.object(network.sys, 'platform', 'linux'), patch('builtins.input', return_value='n'), \
                patch.object(network, 'configure') as configure, contextlib.redirect_stdout(io.StringIO()) as output:
            network.main()
        configure.assert_not_called()
        self.assertIn('KVM user ACLs', output.getvalue())
        self.assertNotIn('vhost-vsock user ACLs', output.getvalue())

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

    def fake_tables(self):
        """iptables/ip6tables state for -C/-D/-S/-F/-X/-N/-A/-I."""
        tables = {'iptables': {}, 'ip6tables': {}}
        builtin = ['INPUT', 'FORWARD', 'OUTPUT']
        for state in tables.values():
            for name in builtin:
                state[name] = []
        def run(tool, wait, op, name, *rule, optional=False):
            state, rule = tables[tool], list(rule)
            found = name in state
            if op == '-C':
                ok = found and rule in state[name]
            elif op == '-D':
                state[name].remove(rule)
                ok = True
            elif op == '-S':
                ok = found
            elif op == '-F':
                state[name].clear()
                ok = True
            elif op == '-X':
                ok = not any(r[-1] == name for chain in state.values() for r in chain)
                del state[name]
            elif op == '-N':
                ok = not found
                state[name] = []
            elif op == '-A':
                state[name].append(rule)
                ok = True
            else:
                state[name].insert(0, rule)
                ok = True
            if not ok and not optional:
                raise RuntimeError(f'{tool} {op} {name} failed')
            return SimpleNamespace(returncode=0 if ok else 1, stdout='')
        return tables, run

    def test_firewall_confines_tap_input_and_relayed_output_idempotently(self):
        n = self.pool[0]
        tables, run = self.fake_tables()
        with patch.object(network, 'run', side_effect=run):
            network.firewall(n)
            network.firewall(n)
        for tool in ['iptables', 'ip6tables']:
            state = tables[tool]
            self.assertEqual(state['INPUT'], [['-i', n['tapName'], '-j', network.chain(n)]])
            self.assertEqual(state['FORWARD'], [['-i', n['tapName'], '-j', 'DROP']])
            self.assertEqual(state[network.chain(n)], network.input_rules(n) if tool == 'iptables' else [['-j', 'DROP']])
        # coturn's relayed packets leave as local output from the relay ports:
        # only the slot's TURN ports are reachable, never other host services.
        ipv4 = tables['iptables']
        self.assertEqual(ipv4['OUTPUT'], [network.relay_hook(n)[1:]])
        self.assertEqual(network.relay_hook(n)[1:7], ['-s', n['hostIp'], '-p', 'udp', '--sport', '49152:49215'])
        self.assertEqual(ipv4[network.relay_chain(n)], [
            ['-d', n['hostIp'], '-p', 'udp', '--dport', '49152:49215', '-j', 'RETURN'],
            ['-d', n['hostIp'], '-p', 'udp', '--dport', '41000', '-j', 'RETURN'],
            ['-j', 'DROP']])
        self.assertNotIn(network.relay_chain(n), tables['ip6tables'])
        with patch.object(network, 'run', side_effect=run):
            network.firewall(n, remove=True)
            network.firewall(n, remove=True)
        for state in tables.values():
            self.assertEqual(sorted(state), ['FORWARD', 'INPUT', 'OUTPUT'])
            self.assertTrue(all(chain == [] for chain in state.values()))

    def test_receipt_without_relay_confinement_is_refused(self):
        self.receipt['policy'] = {key: value for key, value in network.policy(self.pool).items() if key != 'ipv4_relay'}
        with self.assertRaisesRegex(RuntimeError, 'network policy'):
            network.check(self.uid)

    def test_restore_constructs_user_owned_taps_and_receipt(self):
        user = SimpleNamespace(pw_uid=self.uid, pw_gid=1001)
        with patch.object(network, 'write_json') as receipt, patch.object(network.os, 'chown'), patch.object(network, 'trusted_path'):
            network.restore(user)
        self.assertIn(('setfacl', '-m', 'u:1001:rw', '/dev/kvm'), self.calls)
        self.assertIn(('setfacl', '-m', 'u:1001:rw', '/dev/vhost-vsock'), self.calls)
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

    def test_boot_restore_loads_vsock_before_restoring_user_access(self):
        user = SimpleNamespace(pw_uid=self.uid, pw_gid=self.uid)
        exists = Path.exists
        with patch.object(Path, 'exists', lambda p:
                False if str(p) == '/dev/vhost-vsock' else exists(p)), \
                patch.object(network.shutil, 'which', return_value='/usr/sbin/modprobe'), \
                patch.object(network, 'write_json'), patch.object(network.os, 'chown'), \
                patch.object(network, 'trusted_path'):
            network.restore(user)
        module = self.calls.index(('modprobe', 'vhost_vsock'))
        grant = self.calls.index(('setfacl', '-m', 'u:1001:rw', '/dev/vhost-vsock'))
        self.assertLess(module, grant)

    def test_userspace_boot_and_check_preserve_kvm_and_network_without_host_vsock(self):
        self.receipt['transport'] = 'userspace-vsock'
        user = SimpleNamespace(pw_uid=self.uid, pw_gid=self.uid)
        lstat = Path.lstat
        with patch.object(Path, 'lstat', lambda p: SimpleNamespace(st_mode=stat.S_IFREG | 0o755)
                if str(p) == '/usr/bin/bwrap' else lstat(p)), \
                patch.object(network, 'write_json') as receipt, patch.object(network.os, 'chown'), \
                patch.object(network, 'trusted_path'), patch.object(network, 'prepare_vsock_device') as vsock:
            network.restore(user)
            self.assertEqual(network.check(self.uid), self.pool)
            vsock.assert_not_called()
        self.assertIn(('setfacl', '-m', 'u:1001:rw', '/dev/kvm'), self.calls)
        self.assertNotIn(('setfacl', '-m', 'u:1001:rw', '/dev/vhost-vsock'), self.calls)
        self.assertEqual(receipt.call_args.args[1]['transport'], 'userspace-vsock')
        self.assertEqual(receipt.call_args.args[1]['policy'], network.policy(self.pool))

    def test_failed_transport_preparation_leaves_setup_state_absent(self):
        state = network.RUN.parent / 'new-setup-state'
        user = SimpleNamespace(pw_uid=self.uid, pw_gid=self.uid, pw_name='fixture')
        with patch.object(network, 'STATE', state), patch.object(network, 'trusted_path'), \
                patch.object(network.shutil, 'which', return_value='/usr/bin/tool'), \
                patch.object(network, 'prepare_transport', side_effect=RuntimeError('vhost_vsock is unavailable')):
            with self.assertRaisesRegex(RuntimeError, 'vhost_vsock is unavailable'):
                network.configure(user)
        self.assertFalse(state.exists())
        self.assertFalse(any(call[0] == 'setfacl' for call in self.calls))

    def test_foreign_or_tun_devices_remain_untouched_on_restore_and_invalidate(self):
        user = SimpleNamespace(pw_uid=self.uid, pw_gid=self.uid)
        device = network.SYSFS / self.pool[-1]['tapName']
        for name, value in [('owner', str(self.uid + 1)), ('tun_flags', '0x1001')]:
            file = device / name
            previous = file.read_text()
            file.write_text(value)
            for action in [network.restore, network.invalidate]:
                with self.subTest(field=name, action=action.__name__), \
                        patch.object(network, 'trusted_path'), patch.object(network.os, 'chown'):
                    self.calls.clear()
                    with self.assertRaisesRegex(RuntimeError, 'not owned by this user'):
                        action(user)
                    self.assertFalse(any(call[0] in ['ip', 'iptables', 'ip6tables', 'setfacl'] for call in self.calls))
            file.write_text(previous)

    def test_transport_is_explicit_and_a_loaded_nondevice_is_refused(self):
        self.receipt['transport'] = 'automatic'
        with self.assertRaisesRegex(RuntimeError, 'transport is invalid'):
            network.check(self.uid)
        with patch.object(network.os, 'stat', return_value=SimpleNamespace(st_mode=stat.S_IFREG)):
            with self.assertRaisesRegex(RuntimeError, 'character device'):
                network.prepare_vsock_device()

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
            self.assertIn('KVM and vhost-vsock user ACLs', output.getvalue())
            self.assertIn(self.pool[0]['tapName'], output.getvalue())

    def test_setup_and_removal_manage_only_the_account_state(self):
        self.assert_setup_and_removal('kernel-vsock', False)

    def test_missing_kernel_device_and_run_directory_allow_partial_setup_removal(self):
        self.assert_setup_and_removal('kernel-vsock', True)

    def test_userspace_setup_and_removal_use_kvm_only(self):
        self.assert_setup_and_removal('userspace-vsock', True)

    def test_legacy_kvm_acl_setup_persists_transport_before_restore(self):
        self.assert_setup_and_removal('kernel-vsock', False,
                                      {'previous_kvm_acl': 'user:1001:r--'})

    def test_legacy_kvm_and_vsock_acl_setup_persists_transport_before_restore(self):
        self.assert_setup_and_removal('kernel-vsock', False,
                                      {'previous_kvm_acl': 'user:1001:r--',
                                       'previous_vsock_acl': 'user:1001:-w-'})

    def test_automatic_setup_uses_userspace_without_kernel_vsock(self):
        self.assert_setup_and_removal('userspace-vsock', True, automatic=True)

    def test_automatic_setup_records_custom_data_directory_for_repair(self):
        self.assert_setup_and_removal('userspace-vsock', True, automatic=True, custom_data=True)

    def test_automatic_setup_keeps_kernel_vsock(self):
        self.assert_setup_and_removal('kernel-vsock', False, automatic=True)

    def test_automatic_setup_refuses_missing_userspace_prerequisites(self):
        user = SimpleNamespace(pw_uid=self.uid, pw_name='fixture', pw_dir=str(network.RUN.parent))
        lstat = Path.lstat
        def missing_bwrap(path):
            if str(path) == '/usr/bin/bwrap':
                raise FileNotFoundError('/usr/bin/bwrap is missing')
            return lstat(path)
        with patch.object(network, 'STATE', network.RUN.parent / 'absent-state'), \
                patch.object(Path, 'lstat', missing_bwrap), \
                patch.object(network, 'prepare_vsock_device', side_effect=RuntimeError('module unavailable')):
            with self.assertRaisesRegex(RuntimeError, r'Browser transport unavailable:.*vhost.*bwrap.*vhost-device-vsock'):
                network.select_transport(user)
        self.assertFalse((network.RUN.parent / 'absent-state').exists())

    def test_automatic_selection_requires_backend_with_bwrap_present(self):
        user = SimpleNamespace(pw_uid=self.uid, pw_name='fixture', pw_dir=str(network.RUN.parent))
        with patch.object(network, 'STATE', network.RUN.parent / 'absent-state'), \
                patch.object(network, 'prepare_vsock_device', side_effect=RuntimeError('module unavailable')), \
                patch.object(network, 'prepare_transport'):
            with self.assertRaisesRegex(RuntimeError, 'Install the admitted executable .*vhost-device-vsock'):
                network.select_transport(user)

    def test_explicit_transport_override_keeps_the_requested_transport(self):
        user = SimpleNamespace(pw_uid=self.uid)
        with patch.object(network, 'prepare_vsock_device') as kernel:
            for transport in network.TRANSPORTS:
                self.assertEqual(network.select_transport(user, transport), transport)
            kernel.assert_not_called()

    def test_fresh_repair_leaves_transport_to_automatic_setup(self):
        user = SimpleNamespace(pw_uid=self.uid, pw_name='fixture')
        with patch.object(network, 'STATE', network.RUN.parent / 'absent-state'):
            message = network.repair_command(user)
        self.assertNotIn('--transport', message)

    def assert_setup_and_removal(self, transport, missing_vsock, legacy_state=None, automatic=False, custom_data=False):
        root = network.RUN.parent
        unit_dir, udev_dir = root / 'systemd', root / 'udev'
        unit_dir.mkdir()
        udev_dir.mkdir()
        state_dir, helper = root / 'state', root / 'lib/network.py'
        state = state_dir / f'{self.uid}.json'
        if legacy_state is not None:
            state_dir.mkdir()
            state.write_text(json.dumps(legacy_state))
        user = SimpleNamespace(pw_uid=self.uid, pw_gid=self.uid, pw_name='elastos-agent', pw_dir=str(root / 'home'))
        data = root / 'custom Home data' if custom_data else Path(user.pw_dir) / '.local/share/elastos'
        backend = data / 'bin/vhost-device-vsock'
        backend.parent.mkdir(parents=True)
        backend.write_text('fixture backend')
        backend.chmod(0o755)
        legacy_rule = udev_dir / '70-elastos-browser-kvm-1001.rules'
        legacy_rule.write_text('KERNEL=="kvm", RUN+="/usr/bin/setfacl -m u:1001:rw /dev/kvm"\n')
        other_rule = udev_dir / '99-elastos-browser-kvm-1002.rules'
        other_rule.write_text('other Home rule\n')
        def local_path(value):
            if str(value) == '/etc/systemd/system':
                return unit_dir
            if str(value).startswith('/etc/udev/rules.d/'):
                return udev_dir / Path(value).name
            return Path(value)
        real_exists = Path.exists
        real_lstat = Path.lstat
        self.missing_vsock_module = automatic and missing_vsock
        with patch.object(Path, 'exists', lambda path: not self.missing_vsock_module if str(path) == '/dev/vhost-vsock' else real_exists(path)), \
                patch.object(Path, 'lstat', lambda path: SimpleNamespace(st_mode=stat.S_IFREG | 0o755)
                    if str(path) == '/usr/bin/bwrap' else real_lstat(path)), \
                patch.object(network, 'STATE', state_dir), patch.object(network, 'HELPER', helper), \
                patch.object(network, 'Path', side_effect=local_path), patch.object(network, 'trusted_path'), \
                patch.object(network.os, 'chown'), \
                patch.object(network.shutil, 'which', side_effect=lambda command: f'/usr/bin/{command}'), \
                patch.object(network, 'trusted_json', side_effect=lambda file: json.loads(file.read_text())):
            if legacy_state is not None:
                before = state.read_bytes()
                with self.assertRaisesRegex(RuntimeError, 'before changing transport'):
                    network.configure(user, transport='userspace-vsock')
                self.assertEqual(state.read_bytes(), before)
                self.assertFalse(helper.exists())
                self.assertFalse(list(unit_dir.iterdir()))
            if automatic:
                argv = ['network', 'setup', '--user', user.pw_name]
                if custom_data:
                    argv += ['--data-dir', str(data)]
                with patch.object(network.sys, 'argv', argv), \
                        patch.object(network.pwd, 'getpwnam', return_value=user), \
                        patch.object(network.os, 'getuid', return_value=0), \
                        patch.object(network.sys, 'platform', 'linux'), patch('builtins.input', return_value='y'), \
                        contextlib.redirect_stdout(io.StringIO()) as output:
                    network.main()
                self.assertIn(f'transport: {transport}', output.getvalue())
            else:
                network.configure(user, transport=transport)
            self.assertIn(('systemctl', 'restart', 'elastos-browser-network-1001.service'), self.calls)
            unit = unit_dir / 'elastos-browser-network-1001.service'
            self.assertIn('restore --user elastos-agent', unit.read_text())
            self.assertIn('After=network.target nftables.service', unit.read_text())
            services = 'nftables.service firewalld.service ufw.service netfilter-persistent.service'
            self.assertIn(f'PartOf={services}', unit.read_text())
            self.assertIn(f'ReloadPropagatedFrom={services}', unit.read_text())
            self.assertIn('ExecStop=', unit.read_text())
            self.assertIn('ExecStopPost=', unit.read_text())
            self.assertIn('ExecReload=', unit.read_text())
            rule = udev_dir / '99-elastos-browser-kvm-1001.rules'
            self.assertIn('-m u:1001:rw /dev/kvm', rule.read_text())
            if transport == 'kernel-vsock':
                self.assertIn('-m u:1001:rw /dev/vhost-vsock', rule.read_text())
            else:
                self.assertNotIn('/dev/vhost-vsock', rule.read_text())
            self.assertGreater(rule.name, '73-seat-late.rules')
            self.assertFalse(legacy_rule.exists())
            self.assertEqual(other_rule.read_text(), 'other Home rule\n')
            expected = {'previous_kvm_acl': None, **(legacy_state or {}), 'transport': transport, 'data_dir': str(data)}
            if transport == 'kernel-vsock':
                expected.setdefault('previous_vsock_acl', None)
            # Exercise the real setup -> boot restore handoff using the saved
            # state, rather than supplying the expected receipt to restore.
            network.restore(user)
            saved = json.loads(state.read_text())
            receipt = network.RUN / f'{self.uid}.json'
            live = json.loads(receipt.read_text())
            self.assertEqual(saved.get('transport'), live['transport'])
            self.assertEqual(saved, expected)
            self.assertIn(f'--transport {transport}', network.repair_command(user))
            self.assertIn(f'--data-dir {shlex.quote(str(data))}', network.repair_command(user))
            with patch.object(network, 'prepare_vsock_device') as probe:
                self.assertEqual(network.select_transport(user), transport)
                probe.assert_not_called()
            self.assertEqual(live, {**self.receipt, 'transport': transport})
            before = (state.read_bytes(), receipt.read_bytes(), unit.read_bytes(), rule.read_bytes())
            with self.assertRaisesRegex(RuntimeError, 'before changing transport'):
                network.configure(user, transport='userspace-vsock' if transport == 'kernel-vsock' else 'kernel-vsock')
            self.assertEqual((state.read_bytes(), receipt.read_bytes(), unit.read_bytes(), rule.read_bytes()), before)
            self.assertEqual(('modprobe', 'vhost_vsock') in self.calls, automatic and missing_vsock)
            # Model setup failing after its state write but before unit write.
            unit.unlink()
            # Setup can fail before boot restore creates any volatile state.
            # Removal recreates only the trusted locks needed to retire it.
            shutil.rmtree(network.RUN)
            exists = Path.exists
            if missing_vsock:
                self.stack.enter_context(patch.object(Path, 'exists', lambda path:
                    False if str(path) == '/dev/vhost-vsock' else exists(path)))
            legacy_rule.write_text('KERNEL=="kvm", RUN+="/usr/bin/setfacl -m u:1001:rw /dev/kvm"\n')
            def missing_unit(*args, **kwargs):
                if args[:3] == ('systemctl', 'disable', '--now') and not kwargs.get('optional'):
                    raise RuntimeError('Unit file does not exist')
                return self.run_command(*args, **kwargs)
            (network.SYSFS / self.pool[0]['tapName'] / 'flags').write_text('0x0')
            read_text = Path.read_text
            def down_tap(file, *args, **kwargs):
                if file == network.SYSFS / self.pool[0]['tapName'] / 'carrier':
                    raise OSError(errno.EINVAL, 'down TAP')
                return read_text(file, *args, **kwargs)
            with patch.object(network, 'run', side_effect=missing_unit), patch.object(Path, 'read_text', down_tap):
                network.configure(user, remove=True)
            if expected['previous_kvm_acl'] is None:
                self.assertIn(('setfacl', '-x', 'u:1001', '/dev/kvm'), self.calls)
            else:
                self.assertIn(('setfacl', '-m', expected['previous_kvm_acl'], '/dev/kvm'), self.calls)
            if transport == 'kernel-vsock' and not missing_vsock:
                if expected['previous_vsock_acl'] is None:
                    self.assertIn(('setfacl', '-x', 'u:1001', '/dev/vhost-vsock'), self.calls)
                else:
                    self.assertIn(('setfacl', '-m', expected['previous_vsock_acl'], '/dev/vhost-vsock'), self.calls)
            else:
                self.assertNotIn(('setfacl', '-x', 'u:1001', '/dev/vhost-vsock'), self.calls)
            self.assertFalse(unit.exists())
            self.assertEqual(list(udev_dir.iterdir()), [other_rule])
            self.assertEqual(list(state_dir.iterdir()), [])
            self.assertFalse(helper.exists())
            for n in self.pool:
                self.assertIn(('ip', 'tuntap', 'del', 'dev', n['tapName'], 'mode', 'tap'), self.calls)
                self.assertFalse((network.RUN / f"{n['tapName']}.lock").exists())


if __name__ == '__main__':
    unittest.main()
