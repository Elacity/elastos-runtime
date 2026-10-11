#!/usr/bin/env python3
"""Remove one exact path below a directory this code owns, never through a link.

    remove_owned(root, relative, expected=None) -> bool

`root` is the owned directory. Its own last component must be a real
directory, not a link; the folders above it are the caller's trust anchor.
`relative` names the exact path below it in plain components (no absolute
path, "", "." or "..").

The root is opened with O_DIRECTORY|O_NOFOLLOW and every parent below it with
openat(O_DIRECTORY|O_NOFOLLOW), so a folder swapped for a link fails instead of
redirecting the removal elsewhere. The leaf is inspected without following it:
a link is removed itself, a directory is removed entry by entry relative to
open descriptors (never following a link inside it), anything else is
unlinked. With `expected` = (st_dev, st_ino) recorded earlier, a leaf with
another identity is refused.

Returns False when the leaf or one of its parents does not exist and True
after removing it. Raises UnsafeRemoval when a parent is a link or not a
folder, the path is not exact, or the identity changed; OSError otherwise.

Shell form, for scripts that cannot import it:
    python3 scripts/lib/owned_removal.py ROOT RELATIVE [--expect DEV:INO]
exits 0 after removing or when absent, 1 when refused.

scripts/removal-guard.py refuses raw deletion added to product code; this is
the Python and shell helper it names. Rust: elastos_common::owned_removal.
"""
import errno
import os
from pathlib import PurePath
import stat
import sys

DIRECTORY = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC


class UnsafeRemoval(OSError):
    pass


def _names(relative):
    path = PurePath(relative)
    names = path.parts
    if not names or path.is_absolute() or any(name in ("", ".", "..") for name in names) \
            or str(path) != os.fspath(relative).rstrip("/"):
        raise UnsafeRemoval(f"{os.fspath(relative)!r} is not an exact path below the owned root")
    return names


def _open_directory(name, dir_fd=None):
    try:
        return os.open(name, DIRECTORY, dir_fd=dir_fd)
    except OSError as error:
        if error.errno in (errno.ELOOP, errno.ENOTDIR):
            raise UnsafeRemoval(f"{name} is a link or not a folder; nothing was removed") from error
        raise


def _remove_tree(parent, name, identity):
    descriptor = _open_directory(name, parent)
    try:
        opened = os.fstat(descriptor)
        if (opened.st_dev, opened.st_ino) != identity:
            raise UnsafeRemoval(f"{name} changed while it was being removed")
        with os.scandir(descriptor) as entries:
            children = [entry.name for entry in entries]
        for child in children:
            found = os.lstat(child, dir_fd=descriptor)
            if stat.S_ISDIR(found.st_mode):
                _remove_tree(descriptor, child, (found.st_dev, found.st_ino))
            else:
                os.unlink(child, dir_fd=descriptor)
    finally:
        os.close(descriptor)
    os.rmdir(name, dir_fd=parent)


def remove_owned(root, relative, expected=None):
    names = _names(relative)
    try:
        directory = _open_directory(os.fspath(root))
    except FileNotFoundError:
        return False
    try:
        for name in names[:-1]:
            try:
                inner = _open_directory(name, directory)
            except FileNotFoundError:
                return False
            os.close(directory)
            directory = inner
        leaf = names[-1]
        try:
            found = os.lstat(leaf, dir_fd=directory)
        except FileNotFoundError:
            return False
        identity = (found.st_dev, found.st_ino)
        if expected is not None and identity != tuple(expected):
            raise UnsafeRemoval(f"{os.fspath(relative)} is no longer the recorded file; it was kept")
        if stat.S_ISDIR(found.st_mode):
            _remove_tree(directory, leaf, identity)
        else:
            os.unlink(leaf, dir_fd=directory)
        return True
    finally:
        os.close(directory)


def main(argv):
    try:
        if len(argv) not in (2, 4) or (len(argv) == 4 and argv[2] != "--expect"):
            raise ValueError()
        expected = tuple(int(part) for part in argv[3].split(":", 1)) if len(argv) == 4 else None
        if expected is not None and len(expected) != 2:
            raise ValueError()
    except ValueError:
        print("usage: owned_removal.py ROOT RELATIVE [--expect DEV:INO]", file=sys.stderr)
        return 2
    try:
        remove_owned(argv[0], argv[1], expected)
    except OSError as error:
        print(f"Removal refused: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
