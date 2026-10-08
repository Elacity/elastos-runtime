#!/usr/bin/env python3
"""Root-owned Linux Browser TAP pool; check and lease run as the Home user."""
import argparse
import contextlib
import errno
import fcntl
import hashlib
import json
import os
from pathlib import Path
import pwd
import shutil
import shlex
import stat
import subprocess
import sys

RUN = Path('/run/elastos-browser')
STATE = Path('/var/lib/elastos-browser')
HELPER = Path('/usr/local/lib/elastos/browser-vm-linux-network.py')
SYSFS = Path('/sys/class/net')
IPV6 = Path('/proc/sys/net/ipv6/conf')
SLOTS = 4
FIREWALL_SERVICES = 'nftables.service firewalld.service ufw.service netfilter-persistent.service'


class SlotsBusy(RuntimeError):
    pass


def networks(uid):
    digest = hashlib.sha256(str(uid).encode()).digest()
    third, base = 200 + digest[0] % 40, (digest[1] % 16) * 16
    return [dict(tapName=f'ebv{uid:x}s{i}', hostIp=f'192.168.{third}.{base + i * 4 + 1}',
                 guestIp=f'192.168.{third}.{base + i * 4 + 2}', prefix=30, turnPort=41000,
                 mac=f'02:eb:{uid >> 16 & 255:02x}:{uid >> 8 & 255:02x}:{uid & 255:02x}:{i:02x}')
            for i in range(SLOTS)]


def run(*args, optional=False):
    result = subprocess.run(args, text=True, capture_output=True)
    if result.returncode and not optional:
        raise RuntimeError(result.stderr.strip() or f'{args[0]} failed')
    return result


def input_rules(n):
    peer = ['-s', n['guestIp'], '-d', n['hostIp']]
    rules = [peer + ['-m', 'conntrack', '--ctstate', 'ESTABLISHED,RELATED', '-j', 'ACCEPT'],
             peer + ['-p', 'tcp', '--dport', '19091', '-j', 'ACCEPT']]
    for port in [str(n['turnPort']), '49152:49215']:
        for protocol in ['tcp', 'udp']:
            rules.append(peer + ['-p', protocol, '--dport', port, '-j', 'ACCEPT'])
    return rules + [['-j', 'DROP']]


def chain(n):
    return 'EBV_' + n['tapName']


def policy(pool):
    return dict(ipv4=[input_rules(n) for n in pool], ipv6='drop', forward='drop')


def firewall(n, remove=False):
    tap, name = n['tapName'], chain(n)
    for tool in ['iptables', 'ip6tables']:
        jump = ['INPUT', '-i', tap, '-j', name]
        forward = ['FORWARD', '-i', tap, '-j', 'DROP']
        # Own only this pool's chains and hooks; leave host policy intact.
        for rule in [jump, forward]:
            while run(tool, '-w', '-C', *rule, optional=True).returncode == 0:
                run(tool, '-w', '-D', *rule)
        exists = run(tool, '-w', '-S', name, optional=True).returncode == 0
        if exists:
            run(tool, '-w', '-F', name)
        if remove:
            if exists:
                run(tool, '-w', '-X', name)
            continue
        if not exists:
            run(tool, '-w', '-N', name)
        rules = input_rules(n) if tool == 'iptables' else [['-j', 'DROP']]
        for rule in rules:
            run(tool, '-w', '-A', name, *rule)
        run(tool, '-w', '-I', *forward)
        run(tool, '-w', '-I', *jump)


def trusted_path(file):
    for entry in [file, *file.parents]:
        info = entry.lstat()
        if info.st_uid != 0 or info.st_mode & 0o022 or stat.S_ISLNK(info.st_mode):
            raise RuntimeError(f'untrusted setup path: {entry}')


def trusted_json(file):
    trusted_path(file)
    return json.loads(file.read_text())


def boot_id():
    return Path('/proc/sys/kernel/random/boot_id').read_text().strip()


def check(uid):
    receipt = trusted_json(RUN / f'{uid}.json')
    pool = networks(uid)
    if not isinstance(receipt, dict) or receipt.get('uid') != uid or receipt.get('boot_id') != boot_id() or receipt.get('networks') != pool or receipt.get('policy') != policy(pool):
        raise RuntimeError('setup receipt does not match this user, boot or network policy')
    run('systemctl', 'is-active', '--quiet', f'elastos-browser-network-{uid}.service')
    for device in ['/dev/kvm', '/dev/net/tun']:
        if not stat.S_ISCHR(os.stat(device).st_mode) or not os.access(device, os.R_OK | os.W_OK):
            raise RuntimeError(f'{device} requires read/write access')
    if not isinstance(receipt.get('ifindices'), list) or len(receipt['ifindices']) != SLOTS:
        raise RuntimeError('setup receipt must bind all four TAP devices')
    for n, ifindex in zip(pool, receipt['ifindices']):
        sysfs = SYSFS / n['tapName']
        flags = int((sysfs / 'tun_flags').read_text().strip(), 16)
        persistent = 'persist' in run('ip', 'tuntap', 'show', 'dev', n['tapName']).stdout.split()
        if int((sysfs / 'owner').read_text()) != uid or flags & 2 != 2 or not persistent:
            raise RuntimeError('TAP must be persistent and owned by this user')
        addresses = json.loads(run('ip', '-j', 'addr', 'show', 'dev', n['tapName']).stdout)
        link = addresses[0]
        actual = [(a['family'], a['local'], a['prefixlen']) for a in link['addr_info']]
        if link['ifindex'] != ifindex or 'UP' not in link['flags'] or link.get('master') or actual != [('inet', n['hostIp'], 30)]:
            raise RuntimeError('TAP identity, link or address does not match setup')
        if (IPV6 / n['tapName'] / 'disable_ipv6').read_text().strip() != '1':
            raise RuntimeError('TAP IPv6 must be disabled')
    return pool


def active_carrier(tap):
    device = SYSFS / tap
    if not device.exists() or not int((device / 'flags').read_text().strip(), 16) & 1:
        return False
    try:
        return (device / 'carrier').read_text().strip() == '1'
    except OSError as error:
        if error.errno != errno.EINVAL:
            raise
        # The link can go down between reading IFF_UP and carrier.
        return False


@contextlib.contextmanager
def locks(uid, pool):
    with contextlib.ExitStack() as stack:
        for n in pool:
            file = stack.enter_context(open(RUN / f"{n['tapName']}.lock", 'a'))
            fcntl.flock(file, fcntl.LOCK_EX | fcntl.LOCK_NB)
            if active_carrier(n['tapName']):
                raise RuntimeError('Close Browser sessions before changing their network')
        yield


def lease(uid):
    pool = check(uid)
    for n in pool:
        with open(RUN / f"{n['tapName']}.lock", 'r+') as file:
            try:
                fcntl.flock(file, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                continue
            # A VM which outlived its launcher still owns this device.
            if active_carrier(n['tapName']):
                continue
            print(json.dumps(n), flush=True)
            sys.stdin.buffer.read()  # Launcher holds the lease through child cleanup.
            return
    raise SlotsBusy('All four Browser network slots are in use. Close a Browser session and retry.')


def write_json(file, value):
    temporary = file.with_suffix('.new')
    temporary.write_text(json.dumps(value))
    temporary.chmod(0o644)
    temporary.replace(file)


def invalidate(user):
    if RUN.exists():
        trusted_path(RUN)
        (RUN / f'{user.pw_uid}.json').unlink(missing_ok=True)
    # Cut off active guests as well as future launches during firewall changes.
    for n in networks(user.pw_uid):
        if (SYSFS / n['tapName']).exists():
            run('ip', 'link', 'set', n['tapName'], 'down')


def restore(user):
    uid, pool = user.pw_uid, networks(user.pw_uid)
    RUN.mkdir(mode=0o755, exist_ok=True)
    trusted_path(RUN)
    receipt = RUN / f'{uid}.json'
    receipt.unlink(missing_ok=True)
    for n in pool:
        file = RUN / f"{n['tapName']}.lock"
        if not file.exists():
            file.touch(mode=0o600)
        if not stat.S_ISREG(file.lstat().st_mode):
            raise RuntimeError('Browser lease path must be a regular file')
        file.chmod(0o600)
        os.chown(file, uid, user.pw_gid)
    with locks(uid, pool):
        # Refuse unrelated devices and connected routes before any host changes.
        for n in pool:
            device = SYSFS / n['tapName']
            if device.exists() and (not (device / 'owner').exists() or int((device / 'owner').read_text()) != uid):
                raise RuntimeError(f"device name is already owned: {n['tapName']}")
            route = json.loads(run('ip', '-j', 'route', 'show', 'table', 'all').stdout)
            import ipaddress
            subnet = ipaddress.ip_network(f"{n['hostIp']}/30", strict=False)
            for r in route:
                dst = r.get('dst', 'default')
                if dst != 'default' and r.get('dev') != n['tapName'] and subnet.overlaps(ipaddress.ip_network(dst, strict=False)):
                    raise RuntimeError(f'Browser subnet overlaps an existing route: {dst}')
        for n in pool:
            tap = n['tapName']
            # Keep the link down while replacing confinement rules.
            if (SYSFS / tap).exists():
                run('ip', 'link', 'set', tap, 'down')
            else:
                run('ip', 'tuntap', 'add', 'dev', tap, 'mode', 'tap', 'user', str(uid))
            run('sysctl', '-w', f'net.ipv6.conf.{tap}.disable_ipv6=1')
            firewall(n)
            run('ip', 'addr', 'flush', 'dev', tap)
            run('ip', 'addr', 'add', f"{n['hostIp']}/30", 'dev', tap)
            run('ip', 'link', 'set', tap, 'up')
        run('setfacl', '-m', f'u:{uid}:rw', '/dev/kvm')
        write_json(receipt, dict(uid=uid, boot_id=boot_id(), networks=pool, policy=policy(pool),
                                ifindices=[int((SYSFS / n['tapName'] / 'ifindex').read_text()) for n in pool]))


def configure(user, remove=False):
    uid = user.pw_uid
    unit = f'elastos-browser-network-{uid}.service'
    unit_path = Path('/etc/systemd/system') / unit
    udev = Path(f'/etc/udev/rules.d/70-elastos-browser-kvm-{uid}.rules')
    state = STATE / f'{uid}.json'
    trusted_path(unit_path.parent)
    trusted_path(udev.parent)
    for file in [unit_path, udev]:
        if file.exists() or file.is_symlink():
            trusted_path(file)
    if remove:
        if not state.exists():
            raise RuntimeError('Browser network setup state is absent; removal requires its KVM ACL receipt')
        previous = trusted_json(state)['previous_kvm_acl']
        # Refuse removal while any slot has an active owner.
        with locks(uid, networks(uid)):
            (RUN / f'{uid}.json').unlink(missing_ok=True)
            for n in networks(uid):
                present = (SYSFS / n['tapName']).exists()
                if present:
                    run('ip', 'link', 'set', n['tapName'], 'down')
                firewall(n, remove=True)
                if present:
                    run('ip', 'tuntap', 'del', 'dev', n['tapName'], 'mode', 'tap')
            if previous is None:
                run('setfacl', '-x', f'u:{uid}', '/dev/kvm')
            else:
                run('setfacl', '-m', previous, '/dev/kvm')
            run('systemctl', 'disable', '--now', unit, optional=True)
            unit_path.unlink(missing_ok=True)
            udev.unlink(missing_ok=True)
            state.unlink()
        for n in networks(uid):
            (RUN / f"{n['tapName']}.lock").unlink(missing_ok=True)
        if not list(STATE.glob('*.json')):
            HELPER.unlink(missing_ok=True)
    else:
        for command in ['ip', 'iptables', 'ip6tables', 'setfacl', 'getfacl', 'systemctl', 'udevadm']:
            if not shutil.which(command):
                raise RuntimeError(f'Install {command} before Browser network setup')
        STATE.mkdir(mode=0o755, exist_ok=True)
        trusted_path(STATE)
        if not state.exists():
            acl = run('getfacl', '-cpn', '/dev/kvm').stdout.splitlines()
            previous = next((line.split()[0] for line in acl if line.startswith(f'user:{uid}:')), None)
            write_json(state, dict(previous_kvm_acl=previous))
        HELPER.parent.mkdir(mode=0o755, parents=True, exist_ok=True)
        trusted_path(HELPER.parent)
        if HELPER.exists() or HELPER.is_symlink():
            trusted_path(HELPER)
        if Path(__file__).resolve() != HELPER:
            shutil.copyfile(__file__, HELPER)
        HELPER.chmod(0o755)
        udev.write_text(f'KERNEL=="kvm", RUN+="{shutil.which("setfacl")} -m u:{uid}:rw /dev/kvm"\n')
        command = f'{sys.executable} {HELPER}'
        unit_path.write_text(f'[Unit]\nDescription=ElastOS Browser network for UID {uid}\nAfter=network.target {FIREWALL_SERVICES}\nPartOf={FIREWALL_SERVICES}\nReloadPropagatedFrom={FIREWALL_SERVICES}\n\n[Service]\nType=oneshot\nExecStart={command} restore --user {user.pw_name}\nExecReload={command} restore --user {user.pw_name}\nExecStop={command} invalidate --user {user.pw_name}\nExecStopPost={command} invalidate --user {user.pw_name}\nRemainAfterExit=yes\n\n[Install]\nWantedBy=multi-user.target\n')
        run('systemctl', 'daemon-reload')
        run('systemctl', 'enable', unit)
    run('systemctl', 'daemon-reload')
    run('udevadm', 'control', '--reload-rules')
    if not remove:
        run('systemctl', 'restart', unit)


def repair_command(user):
    prerequisite = ''
    try:
        trusted_path(HELPER)
        if not stat.S_ISREG(HELPER.lstat().st_mode):
            raise RuntimeError('helper is not a regular file')
    except (OSError, RuntimeError):
        prerequisite = f'Install the reviewed root-owned helper at {HELPER} first. '
    return f'{prerequisite}As root, run: python3 {shlex.quote(str(HELPER))} setup --user {shlex.quote(user.pw_name)}'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['setup', 'remove', 'restore', 'invalidate', 'check', 'lease'])
    parser.add_argument('--user')
    args = parser.parse_args()
    user = pwd.getpwnam(args.user) if args.user else pwd.getpwuid(os.getuid())
    if args.action in ['check', 'lease']:
        if user.pw_uid != os.getuid() or os.getuid() == 0:
            raise RuntimeError('Run Browser as its ordinary Home user')
        try:
            if args.action == 'lease':
                lease(user.pw_uid)
            else:
                print(json.dumps(check(user.pw_uid)))
        except SlotsBusy:
            raise
        except (OSError, ValueError, KeyError, IndexError, TypeError, RuntimeError) as error:
            raise RuntimeError(f'Browser network is unavailable ({error}). {repair_command(user)}') from error
        return
    if os.getuid() != 0 or not args.user or user.pw_uid == 0:
        raise RuntimeError('Run setup/removal as root with --user naming the ordinary Home account')
    if sys.platform != 'linux':
        raise RuntimeError('Browser network setup requires a Linux host')
    if args.action in ['setup', 'remove']:
        verb = 'Remove' if args.action == 'remove' else 'Create/repair'
        print(f'{verb} four persistent user-owned TAPs, private /30 addresses, IPv4/IPv6 confinement rules, KVM user ACL, a KVM udev rule and a boot service for {user.pw_name}.\nThe host keeps its other firewall rules. Close Browser sessions first.')
        for n in networks(user.pw_uid):
            print(f"  {n['tapName']}: host {n['hostIp']}/30, guest {n['guestIp']}; relay TCP 19091, TURN TCP/UDP 41000 and 49152:49215")
        if input('Continue? [y/N] ').lower() != 'y':
            return
        configure(user, remove=args.action == 'remove')
    else:
        invalidate(user)
        if args.action == 'restore':
            restore(user)


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, KeyError, RuntimeError) as error:
        print(error, file=sys.stderr)
        sys.exit(1)
