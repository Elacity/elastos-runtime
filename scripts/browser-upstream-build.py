#!/usr/bin/env python3
"""Prepare pinned Browser host inputs on the native release runner, before signing."""
import io
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tarfile
import tempfile


def source_header(member):
    member.uid = member.gid = member.mtime = 0
    member.uname = member.gname = ''
    return member


def validate(recipe, upstream):
    build = recipe['build']
    if build.get('kind') in ('e2fsprogs-static-v1', 'crosvm-static-v1', 'python-standalone-v1'):
        component = {'e2fsprogs-static-v1': 'debugfs', 'crosvm-static-v1': 'crosvm',
                     'python-standalone-v1': 'python3'}[build['kind']]
        names = {'debugfs': [], 'crosvm': ['minijail', 'libcap'], 'python3': ['python-licenses']}[component]
        if (recipe['component'] != component or set(build) != {'kind', 'dependencies'}
                or [d.get('name') for d in build['dependencies']] != names):
            raise ValueError('unsupported Browser native source recipe')
        for dep in build['dependencies']:
            if set(dep) != {'name', 'root', 'source'} or '/' in upstream.relative(dep['root']):
                raise ValueError('invalid source dependency')
            upstream.source_spec(dep['source'])
        if component == 'crosvm' and (recipe['platform'] != 'linux-arm64'
                or not {'licenses/minijail.txt', 'licenses/libcap.txt'} <= {
                    f['name'] for f in recipe['license']['files']}):
            raise ValueError('crosvm requires its Linux ARM64 recipe and dependency licences')
        return
    if (recipe['component'] != 'turnserver' or build.get('kind') != 'coturn-static-v1'
            or set(build) != {'kind', 'dependencies'} or len(build['dependencies']) != 2):
        raise ValueError('unsupported source build recipe')
    if [d['name'] for d in build['dependencies']] != ['openssl', 'libevent']:
        raise ValueError('TURN requires pinned OpenSSL and libevent')
    for dep in build['dependencies']:
        if set(dep) != {'name', 'root', 'source'} or '/' in upstream.relative(dep['root']):
            raise ValueError('invalid source dependency')
        upstream.source_spec(dep['source'])
    licenses = {f['name'] for f in recipe['license']['files']}
    if not {'LICENSE', 'licenses/openssl.txt', 'licenses/libevent.txt'} <= licenses:
        raise ValueError('TURN requires dependency licences')


def extract(source, root, destination, upstream):
    # Validate first; materialize file aliases as copies, without following links.
    members = upstream.archive_members(source, 'tar.gz', root, 1024**3,
                                       source_archive=True, max_files=8192)
    with tarfile.open(source, 'r:gz') as archive:
        for name, info in members.items():
            path = destination / name
            path.parent.mkdir(parents=True, exist_ok=True)
            with path.open('xb') as target, archive.extractfile(info['source']) as stream:
                shutil.copyfileobj(stream, target)
            path.chmod(info['mode'])
    return destination / root


def audit_libraries(text, darwin):
    if not darwin:
        if text.strip() in ('statically linked', 'not a dynamic executable'):
            return
        raise ValueError('Linux Browser helpers must statically link their runtime and third-party libraries')
    libraries = [line.strip().split(' (', 1)[0] for line in text.splitlines()[1:] if line.strip()]
    if not libraries or not all(name.startswith(('/usr/lib/', '/System/Library/Frameworks/')) for name in libraries):
        raise ValueError('Browser helpers must link third-party libraries statically: ' + ', '.join(libraries))


def build(recipe, payload, cache, upstream):
    validate(recipe, upstream)
    if recipe['build']['kind'] == 'python-standalone-v1':
        return package_python_licenses(recipe, payload, cache, upstream)
    if recipe['build']['kind'] != 'coturn-static-v1':
        return build_browser_native(recipe, payload, cache, upstream)
    native = ('darwin' if platform.system() == 'Darwin' else 'linux') + '-' + (
        'arm64' if platform.machine() in ('arm64', 'aarch64') else 'amd64')
    if recipe['platform'] != native:
        raise ValueError('TURN source build requires its native platform runner')
    tools = ('perl', 'make', 'aclocal', 'autoconf', 'automake',
             'glibtoolize' if native.startswith('darwin') else 'libtoolize',
             'cc' if native.startswith('darwin') else 'musl-gcc')
    missing = [name for name in tools if not shutil.which(name)]
    if missing:
        raise ValueError('TURN build needs host tools before compilation: ' + ', '.join(missing))
    # All inputs are admitted before configure. Configure/make have no fetch step.
    inputs = [(recipe['root'], payload)] + [(d['root'], upstream.cached_input(d['source'], cache))
                                           for d in recipe['build']['dependencies']]
    for item in recipe['license']['files']:
        upstream.cached_input(item['source'], cache, upstream.METADATA_BYTES)
    upstream.disk_gate(cache, 3 * 1024**3)
    with tempfile.TemporaryDirectory(prefix='turn-build-', dir=cache) as scratch:
        work = Path(scratch)
        sources = [extract(path, root, work / 'src', upstream) for root, path in inputs]
        turn, openssl, event = sources
        prefix = work / 'static/usr'
        jobs = str(min(os.cpu_count() or 2, 4))
        env = {**os.environ, 'PKG_CONFIG_PATH': str(prefix / 'lib/pkgconfig'),
               'PKG_CONFIG_LIBDIR': str(prefix / 'lib/pkgconfig'),
               'PKG_CONFIG_SYSROOT_DIR': str(work / 'static'),
               'ELASTOS_BROWSER_BUILD_ROOT': str(work), 'TURN_DISABLE_RPATH': '1',
               'CPPFLAGS': '-I' + str(prefix / 'include'),
               'LDFLAGS': '-L' + str(prefix / 'lib'), 'LC_ALL': 'C',
               'CFLAGS': '-O2 -g0 -ffile-prefix-map=' + str(work) + '=/usr/src/elastos-browser'}
        if native.startswith('linux'):
            env['CC'] = 'musl-gcc'
            env['LDFLAGS'] += ' -static'
        for key in ('CPATH', 'LIBRARY_PATH', 'DYLD_LIBRARY_PATH', 'LD_LIBRARY_PATH', 'LD_PRELOAD'):
            env.pop(key, None)
        # OpenSSL encodes compiler flags as characters in its version metadata.
        # Preserve the flags while replacing the private build-root display.
        build_info = openssl / 'util/mkbuildinf.pl'
        original = build_info.read_text()
        marker = "my $cflags = join(' ', @ARGV);"
        if original.count(marker) != 1:
            raise ValueError('pinned OpenSSL build-information marker changed')
        build_info.write_text(original.replace(marker, marker + '\n'
            '# ElastOS removes private build roots from informational compiler flags.\n'
            '$cflags =~ s/\\Q$ENV{ELASTOS_BROWSER_BUILD_ROOT}\\E/\\/usr\\/src\\/elastos-browser/g;'))
        def run(args, cwd):
            subprocess.run(args, cwd=cwd, env=env, check=True)
        target = {'darwin-arm64': 'darwin64-arm64-cc', 'linux-arm64': 'linux-aarch64',
                  'linux-amd64': 'linux-x86_64'}[native]
        run(['perl', './Configure', target, '--prefix=/usr', '--libdir=lib', '--openssldir=/etc/ssl',
             'no-shared', 'no-module', 'no-async', 'no-tests', 'no-zlib'], openssl)
        run(['make', '-j' + jobs], openssl)
        run(['make', 'install_sw', 'DESTDIR=' + str(work / 'static')], openssl)
        run(['./autogen.sh'], event)
        run(['./configure', '--prefix=/usr', '--disable-shared', '--enable-static',
             '--enable-openssl', '--disable-samples', '--disable-libevent-regress'], event)
        run(['make', '-j' + jobs], event)
        run(['make', 'install', 'DESTDIR=' + str(work / 'static')], event)
        run(['./configure', '--prefix=/usr', '--disable-shared',
             '--disable-sqlite', '--disable-mysql', '--disable-redis', '--disable-postgresql',
             '--disable-mongodb', '--disable-prometheus'], turn)
        run(['make', '-j' + jobs], turn)
        run(['make', 'install', 'DESTDIR=' + str(work / 'install')], turn)
        binary = work / 'install/usr/bin/turnserver'
        upstream.regular(binary)
        if str(work).encode() in binary.read_bytes():
            raise ValueError('TURN binary contains its private build path')
        audit = subprocess.run(['otool', '-L', str(binary)] if native.startswith('darwin') else ['ldd', str(binary)],
                               capture_output=True, text=True)
        audit_libraries(audit.stdout + audit.stderr, native.startswith('darwin'))
        if audit.returncode and 'not a dynamic executable' not in audit.stderr:
            raise ValueError('native library audit failed')
        # Verify after relocation, with build-time search paths removed.
        relocated = work / 'relocated/bin/turnserver'
        relocated.parent.mkdir(parents=True)
        shutil.copy2(binary, relocated)
        subprocess.run([str(relocated), '--version'], env=env, check=True, capture_output=True)
        output = cache / ('turnserver-built-' + native + '.tar.gz')
        staged = work / 'built.tar.gz'
        with tarfile.open(staged, 'x:gz', format=tarfile.GNU_FORMAT) as archive:
            archive.add(relocated, arcname=recipe['root'] + '/bin/turnserver')
            data = (audit.stdout + audit.stderr).replace(str(binary), 'turnserver').encode()
            member = tarfile.TarInfo(recipe['root'] + '/LIBRARY-AUDIT.txt')
            member.size, member.mode = len(data), 0o644
            archive.addfile(member, io.BytesIO(data))
            for root, path in inputs:
                archive.add(path, arcname=recipe['root'] + '/sources/' + root + '.tar.gz')
        os.link(staged, output)
        return output


def package_python_licenses(recipe, payload, cache, upstream):
    """Keep the install-only bytes and add the pinned distribution's full notices."""
    full = upstream.cached_input(recipe['build']['dependencies'][0]['source'], cache)
    metadata, licenses = None, {}
    with subprocess.Popen(['zstd', '-dc', str(full)], stdout=subprocess.PIPE) as process:
        with tarfile.open(fileobj=process.stdout, mode='r|') as archive:
            for member in archive:
                if member.name != 'python/PYTHON.json' and not member.name.startswith('python/licenses/'):
                    continue
                if member.isdir():
                    continue
                relative = upstream.relative(member.name)
                if not member.isfile() or member.size > upstream.METADATA_BYTES:
                    raise ValueError('Python licence metadata must be bounded regular files')
                with archive.extractfile(member) as stream:
                    data = stream.read(upstream.METADATA_BYTES + 1)
                if relative == 'python/PYTHON.json':
                    if metadata is not None:
                        raise ValueError('Duplicate Python build metadata')
                    metadata = json.loads(data)
                elif relative in licenses:
                    raise ValueError('Duplicate Python licence notice')
                else:
                    licenses[relative] = data
        while process.stdout.read(1024 * 1024):
            pass
        if process.wait():
            raise ValueError('Python licence archive decompression failed')
    for item in recipe['license']['files']:
        if item['name'].startswith('licenses/') and 'source' in item:
            name = 'python/' + item['name']
            data = upstream.cached_input(item['source'], cache, upstream.METADATA_BYTES).read_bytes()
            if name in licenses and licenses[name] != data:
                raise ValueError('Python distribution notice differs from its supplemental pin')
            licenses[name] = data
    triple = {'darwin-arm64': 'aarch64-apple-darwin', 'linux-arm64': 'aarch64-unknown-linux-gnu',
              'linux-amd64': 'x86_64-unknown-linux-gnu'}[recipe['platform']]
    if (not isinstance(metadata, dict) or metadata.get('target_triple') != triple
            or metadata.get('python_version') != recipe['version'] or not licenses):
        raise ValueError('Python licence metadata has a different version or target')
    def required_notices(value):
        if isinstance(value, dict):
            for key, entry in value.items():
                if key == 'license_path' and isinstance(entry, str):
                    yield 'python/' + entry
                elif key == 'license_paths' and isinstance(entry, list):
                    yield from ('python/' + path for path in entry)
                else:
                    yield from required_notices(entry)
        elif isinstance(value, list):
            for entry in value:
                yield from required_notices(entry)
    if not set(required_notices(metadata)) <= set(licenses):
        raise ValueError('Python distribution is missing a required licence notice')
    upstream.disk_gate(cache, payload.stat().st_size + sum(map(len, licenses.values())))
    members = upstream.archive_members(payload, 'tar.gz', 'python', recipe['max_unpacked_bytes'],
                                       source_archive=True, max_files=8192)
    # Browser uses Python for noninteractive helpers. The terminal database
    # carries Linux case aliases and is outside that runtime dependency closure.
    members = {name: info for name, info in members.items()
               if not name.startswith('python/share/terminfo/')}
    output = cache / ('python3-built-' + recipe['platform'] + '.tar.gz')
    with tempfile.TemporaryDirectory(prefix='python-notices-', dir=cache) as scratch:
        staged = Path(scratch) / 'built.tar.gz'
        with tarfile.open(payload, 'r:gz') as original, tarfile.open(staged, 'x:gz', format=tarfile.GNU_FORMAT) as archive:
            for name, info in members.items():
                member = tarfile.TarInfo(name)
                member.size, member.mode = info['size'], info['mode']
                with original.extractfile(info['source']) as stream:
                    archive.addfile(member, stream)
            additions = {**licenses, 'python/PYTHON-BUILD.json': upstream.canonical(metadata)}
            if set(additions) & set(members):
                raise ValueError('Python licence notices collide with install payload')
            for name, data in additions.items():
                member = tarfile.TarInfo(name)
                member.size, member.mode = len(data), 0o644
                archive.addfile(member, io.BytesIO(data))
        os.link(staged, output)
    return output


def build_browser_native(recipe, payload, cache, upstream):
    native = ('darwin' if platform.system() == 'Darwin' else 'linux') + '-' + (
        'arm64' if platform.machine() in ('arm64', 'aarch64') else 'amd64')
    if recipe['platform'] != native:
        raise ValueError('Browser source build requires its native platform runner')
    inputs = [(recipe['root'], payload)] + [(d['root'], upstream.cached_input(d['source'], cache))
                                           for d in recipe['build']['dependencies']]
    for item in recipe['license']['files']:
        upstream.cached_input(item['source'], cache, upstream.METADATA_BYTES)
    upstream.disk_gate(cache, 3 * 1024**3)
    with tempfile.TemporaryDirectory(prefix='browser-native-build-', dir=cache) as scratch:
        work = Path(scratch)
        source = extract(payload, recipe['root'], work / 'src', upstream)
        env = {**os.environ, 'LC_ALL': 'C', 'PKG_CONFIG_PATH': '', 'PKG_CONFIG_LIBDIR': '',
               'CFLAGS': '-O2 -g0 -ffile-prefix-map=' + str(work) + '=/usr/src/elastos-browser'}
        for key in ('CPATH', 'LIBRARY_PATH', 'DYLD_LIBRARY_PATH', 'LD_LIBRARY_PATH', 'LD_PRELOAD'):
            env.pop(key, None)
        jobs = str(min(os.cpu_count() or 2, 4))
        if native.startswith('linux'):
            env.update(CC='musl-gcc', CXX='musl-g++', LDFLAGS='-static')
        binaries = {}
        if recipe['component'] == 'debugfs':
            subprocess.run(['./configure', '--disable-nls', '--disable-elf-shlibs',
                            '--disable-bsd-shlibs', '--disable-fuse2fs', '--disable-uuidd',
                            '--enable-libuuid', '--enable-libblkid'], cwd=source, env=env, check=True)
            subprocess.run(['make', '-j' + jobs, 'libs'], cwd=source, env=env, check=True)
            for directory, names in [('debugfs', ['debugfs'])]:
                subprocess.run(['make', '-j' + jobs, *names], cwd=source / directory, env=env, check=True)
                binaries.update({name: source / directory / name for name in names})
        else:
            dependency_root, dependency = inputs[1]
            minijail = extract(dependency, dependency_root, work / 'dependencies', upstream)
            destination = source / 'third_party/minijail'
            if destination.exists():
                destination.rmdir()  # Source archives omit the pinned submodule content.
            shutil.move(minijail, destination)
            for key in ('MINIJAIL_DEFAULT_RET_LOG', 'MINIJAIL_DO_NOT_BUILD', 'MINIJAIL_BINDGEN_TARGET',
                        'COMPILE_SECCOMP_POLICY', 'SECCOMP_DEFAULT_RET_LOG', 'USE_seccomp',
                        'USE_LIBC_COMPATIBILITY_ALLOWLIST', 'MAKEFLAGS', 'MFLAGS', 'CARGO_ENCODED_RUSTFLAGS'):
                env.pop(key, None)
            if shutil.which('compile_seccomp_policy', path=env.get('PATH')):
                raise ValueError('crosvm build PATH must leave seccomp compilation to pinned Minijail')
            env['CROSS_COMPILE'] = ''
            # Keep musl's standard headers first. Supply only Linux UAPI from
            # the native build toolchain, without adding glibc's include root.
            headers = Path(env.pop('ELASTOS_BROWSER_LINUX_UAPI_INCLUDE', '/usr/include'))
            uapi = work / 'linux-uapi'
            for name in ('linux', 'asm-generic', 'asm'):
                origin = headers / name
                if name == 'asm' and not origin.is_dir():
                    origin = headers / 'aarch64-linux-gnu/asm'
                if not origin.is_dir():
                    raise ValueError('crosvm build needs Linux UAPI headers: ' + name)
                shutil.copytree(origin, uapi / name)
            libcap = extract(inputs[2][1], inputs[2][0], work / 'dependencies', upstream)
            subprocess.run(['make', '-C', str(libcap / 'libcap'), '-j' + jobs, 'libcap.a',
                            'CC=' + env['CC'], 'BUILD_CC=cc', 'SHARED=no', 'GOLANG=no',
                            'PAM_CAP=no', 'USE_GPERF=no', 'CFLAGS=' + env['CFLAGS'] + ' -fPIC'],
                           env=env, check=True)
            env.update(MINIJAIL_DIR=str(destination),
                       CPPFLAGS='-I' + str(libcap / 'libcap/include') + ' -I' + str(libcap / 'libcap/include/uapi')
                                + ' -idirafter ' + str(uapi),
                       LDFLAGS='-L' + str(libcap / 'libcap'))
            env['BINDGEN_EXTRA_CLANG_ARGS'] = env['CPPFLAGS']
            env['MINIJAIL_BINDGEN_TARGET'] = 'aarch64-unknown-linux-musl'
            # Minijail's Cargo producer otherwise builds its shared launchers
            # too. The Browser links only its pinned static library.
            build_script = destination / 'rust/minijail-sys/build.rs'
            original = build_script.read_text()
            marker = '.arg(&jobs)\n            .status()?;'
            if original.count(marker) != 1:
                raise ValueError('pinned Minijail static-build marker changed')
            build_script.write_text(original.replace(marker,
                '.arg(&jobs)\n            // ElastOS builds the static Browser host library only.\n'
                '            .arg("CC_STATIC_LIBRARY(libminijail.pic.a)")\n            .status()?;'))
            # Cargo.lock pins the registry closure. Fetch once at build time,
            # retain its sources/licences, then compile with network disabled.
            # The pinned tree owns vendor/generic path crates. Keep Cargo's
            # replaceable registry output separate from those source members.
            vendor = subprocess.run(['cargo', '+1.91.0', 'vendor', '--locked', '--versioned-dirs', 'registry-vendor'],
                                    cwd=source, env=env, check=True, capture_output=True, text=True)
            config = source / '.cargo/config.toml'
            config.parent.mkdir(exist_ok=True)
            with config.open('a') as output:
                output.write('\n' + vendor.stdout)
            target = 'aarch64-unknown-linux-musl'
            env.pop('CARGO_TARGET_DIR', None)
            env.pop('CARGO_BUILD_BUILD_DIR', None)
            env.update(CROSS_COMPILE='', RUSTFLAGS='-C target-feature=+crt-static --remap-path-prefix='
                       + str(work) + '=/usr/src/elastos-browser -L native=' + str(libcap / 'libcap'))
            env['CC_aarch64_unknown_linux_musl'] = env['CC']
            env['CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER'] = env['CC']
            subprocess.run(['cargo', '+1.91.0', 'build', '--locked', '--offline', '--release',
                            '--target', target, '--no-default-features', '--features', 'net',
                            '--bin', 'crosvm', '-j', jobs], cwd=source, env=env, check=True)
            binaries['crosvm'] = source / 'target' / target / 'release/crosvm'
        audit_records = []
        for name, binary in binaries.items():
            upstream.regular(binary)
            if str(work).encode() in binary.read_bytes():
                raise ValueError('Browser binary contains its private build path: ' + name)
            result = subprocess.run(['otool', '-L', str(binary)] if native.startswith('darwin')
                                    else ['ldd', str(binary)], capture_output=True, text=True)
            audit_libraries(result.stdout + result.stderr, native.startswith('darwin'))
            audit_records.append((result.stdout + result.stderr).replace(str(binary), name))
            relocated = work / 'relocated/bin' / name
            relocated.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(binary, relocated)
            # debugfs -V reports version; crosvm supports --version.
            check = subprocess.run([str(relocated), '--version' if name == 'crosvm' else '-V'],
                                   env=env, capture_output=True, text=True)
            accepted = (0,) if name == 'crosvm' else (0, 1)
            if check.returncode not in accepted or not (check.stdout + check.stderr).strip():
                raise ValueError('Browser native relocation probe failed: ' + name)
        output = cache / (recipe['component'] + '-built-' + native + '.tar.gz')
        staged = work / 'built.tar.gz'
        if recipe['component'] == 'crosvm':
            vendor_sources = work / 'cargo-vendor.tar.gz'
            with tarfile.open(vendor_sources, 'x:gz', format=tarfile.GNU_FORMAT) as archive:
                archive.add(source / 'registry-vendor', arcname='registry-vendor', filter=source_header)
            header_sources = work / 'linux-uapi.tar.gz'
            with tarfile.open(header_sources, 'x:gz', format=tarfile.GNU_FORMAT) as archive:
                archive.add(uapi, arcname='linux-uapi', filter=source_header)
        with tarfile.open(staged, 'x:gz', format=tarfile.GNU_FORMAT) as archive:
            for name in binaries:
                archive.add(work / 'relocated/bin' / name, arcname=recipe['root'] + '/bin/' + name)
            data = ''.join(audit_records).encode()
            member = tarfile.TarInfo(recipe['root'] + '/LIBRARY-AUDIT.txt')
            member.size, member.mode = len(data), 0o644
            archive.addfile(member, io.BytesIO(data))
            for root, path in inputs:
                archive.add(path, arcname=recipe['root'] + '/sources/' + root + '.tar.gz')
            if recipe['component'] == 'crosvm':
                archive.add(vendor_sources, arcname=recipe['root'] + '/sources/cargo-vendor.tar.gz')
                archive.add(header_sources, arcname=recipe['root'] + '/sources/linux-uapi.tar.gz')
                archive.add(source / 'Cargo.lock', arcname=recipe['root'] + '/sources/Cargo.lock')
        os.link(staged, output)
        return output
