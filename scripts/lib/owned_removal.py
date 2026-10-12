#!/usr/bin/env python3
"""Remove one exact path below a directory this code owns, never through a link.

    remove_owned(root, relative, expected=None) -> bool

`root` is the owned directory. The folders above it are the caller's trust
anchor; its own last name must be a real directory, not a link, however the
root is spelled (`data`, `data/` and `data/.` all name `data` itself).
`relative` names the exact path below it in plain components (no absolute
path, "", "." or "..").

The root's parent is opened, then the root's last name with
openat(O_DIRECTORY|O_NOFOLLOW), then every folder below it the same way, so a
folder that is a link fails instead of redirecting the removal elsewhere. The
leaf is inspected with lstat relative to its parent's descriptor: a link is
removed itself, a directory is emptied through descriptors opened the same way
(never following a link inside it), anything else is unlinked. Every unlink
and rmdir is relative to a held directory descriptor, so renaming or linking a
folder on the path after the walk cannot redirect it. With `expected` =
(st_dev, st_ino) recorded earlier, a leaf with another identity is refused.

Accepted limit: POSIX removes a name, not an inode. Between the identity check
and the unlink, and while a folder is being emptied, a process running as the
same user can still rename entries inside the owned folder. The owned folder
is that user's own private (0700) directory, so no other account can; this is
the same limit recorded for #183.

Returns False when the leaf or one of its folders does not exist and True
after removing it. Raises UnsafeRemoval when a folder is a link or not a
folder, the path is not exact, or the identity changed; OSError otherwise.

Shell and Node form, for code that cannot import it:
    python3 scripts/lib/owned_removal.py ROOT RELATIVE [--expect DEV:INO]
exits 0 after removing or when absent, 1 when refused or failed, 2 on usage.

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


def _open_directory(name, dir_fd):
    try:
        return os.open(name, DIRECTORY, dir_fd=dir_fd)
    except OSError as error:
        if error.errno in (errno.ELOOP, errno.ENOTDIR):
            raise UnsafeRemoval(f"{name} is a link or not a folder; nothing was removed") from error
        raise


def _open_root(root):
    """The owned root's descriptor, its last name opened without following it."""
    text = os.fsdecode(root)
    names = [name for name in text.split("/") if name not in ("", ".")]
    if not names or names[-1] == "..":
        raise UnsafeRemoval(f"{text!r} does not name the owned folder itself")
    anchor = "/".join(names[:-1])
    anchor = ("/" + anchor) if text.startswith("/") else (anchor or ".")
    parent = os.open(anchor, os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
    try:
        return _open_directory(names[-1], parent)
    finally:
        os.close(parent)


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
        directory = _open_root(root)
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
