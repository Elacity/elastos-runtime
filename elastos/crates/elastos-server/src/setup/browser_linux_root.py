"""Fixed Runtime bootstrap: stdin carries data, never privileged Python code."""
import hashlib
import os
import re
import stat
import sys
import uuid

ROOT_HELPER = '/usr/local/lib/elastos/browser-vm-linux-network.py'
MAX_HELPER_BYTES = 1024 * 1024


def trusted_directory(path, owner):
    """Walk from / through owned, non-writable directories without following links."""
    directory = os.open('/', os.O_RDONLY | os.O_DIRECTORY)
    try:
        metadata = os.fstat(directory)
        if metadata.st_uid != owner or metadata.st_mode & 0o022:
            raise RuntimeError('Browser helper requires root-owned directories')
        for part in path.strip('/').split('/'):
            try:
                os.mkdir(part, mode=0o755, dir_fd=directory)
            except FileExistsError:
                pass
            child = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                            dir_fd=directory)
            os.close(directory)
            directory = child
            metadata = os.fstat(directory)
            if metadata.st_uid != owner or metadata.st_mode & 0o022:
                raise RuntimeError('Browser helper requires root-owned directories')
        return directory
    except BaseException:
        os.close(directory)
        raise


def verified_copy(directory, name, expected, payload, owner):
    """Replace stale copies atomically; return bytes read back from the owned inode."""
    if not re.fullmatch(r'[0-9a-f]{64}', expected):
        raise RuntimeError('Browser helper requires its admitted SHA-256')
    if len(payload) > MAX_HELPER_BYTES or hashlib.sha256(payload).hexdigest() != expected:
        raise RuntimeError('Browser helper differs from its admitted SHA-256')
    metadata = os.fstat(directory)
    if metadata.st_uid != owner or metadata.st_mode & 0o022:
        raise RuntimeError('Browser helper requires a root-owned directory')
    temporary = '.browser-helper-' + uuid.uuid4().hex
    fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                 0o600, dir_fd=directory)
    try:
        with os.fdopen(fd, 'wb') as output:
            output.write(payload)
            output.flush()
            os.fchmod(output.fileno(), 0o644)
            os.fsync(output.fileno())
        os.rename(temporary, name, src_dir_fd=directory, dst_dir_fd=directory)
    finally:
        try:
            os.unlink(temporary, dir_fd=directory)
        except FileNotFoundError:
            pass
    fd = os.open(name, os.O_RDONLY | os.O_NOFOLLOW, dir_fd=directory)
    with os.fdopen(fd, 'rb') as installed:
        metadata = os.fstat(installed.fileno())
        if (not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != owner
                or metadata.st_mode & 0o022):
            raise RuntimeError('Browser helper requires a root-owned regular file')
        verified = installed.read(MAX_HELPER_BYTES + 1)
    if hashlib.sha256(verified).hexdigest() != expected:
        raise RuntimeError('Root Browser helper differs from its admitted SHA-256')
    return verified


def main():
    if os.geteuid() != 0:
        raise RuntimeError('Browser host setup requires root')
    expected, user, data, action = sys.argv[1:]
    if action not in ('install', 'setup'):
        raise RuntimeError('Invalid Browser host action')
    payload = sys.stdin.buffer.read(MAX_HELPER_BYTES + 1)
    directory = trusted_directory(os.path.dirname(ROOT_HELPER), 0)
    try:
        verified = verified_copy(directory, os.path.basename(ROOT_HELPER), expected, payload, 0)
    finally:
        os.close(directory)
    if action == 'install':
        return
    # sudo reads its password from the controlling tty. The helper keeps its own
    # confirmation on that tty after the payload pipe reaches EOF.
    sys.stdin = open('/dev/tty', encoding='utf-8')
    sys.argv = [ROOT_HELPER, 'setup', '--user', user, '--data-dir', data]
    # Compile the bytes read from the verified root inode, without reopening a path.
    exec(compile(verified, ROOT_HELPER, 'exec'),
         {'__name__': '__main__', '__file__': ROOT_HELPER})


if __name__ == '__main__':
    main()
