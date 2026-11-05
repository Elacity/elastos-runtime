#!/usr/bin/env python3
"""Remove one exact path below a directory this code owns, never through a link.

    remove_owned(root, relative, expected=None) -> bool

`root` is the owned directory. Its absolute path and every
parent below it are opened one component at a time without following links.
Relative roots use the current directory descriptor as their trust anchor.
`relative` names the exact path below it in plain components (no absolute
path, "", "." or "..").

The root is opened with O_DIRECTORY|O_NOFOLLOW and every parent below it with
openat(O_DIRECTORY|O_NOFOLLOW), so a folder swapped for a link fails instead of
redirecting the removal elsewhere. The leaf is inspected without following it:
a link is removed itself, a directory is removed entry by entry relative to
open descriptors (never following a link inside it), anything else is
unlinked. With `expected` = (st_dev, st_ino) recorded earlier, a leaf with
another identity is refused. Expected files are first moved to a private
holding directory, then compared and removed there. A replacement at the
original name survives. A changed captured file is restored without overwrite;
a restore conflict is retained in the holding directory for repair.

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
import secrets
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


def _open_root(root):
    path = PurePath(root)
    if ".." in path.parts:
        raise UnsafeRemoval("the removal root contains a parent traversal")
    directory = _open_directory("/" if path.is_absolute() else ".")
    try:
        for name in path.parts[1:] if path.is_absolute() else path.parts:
            inner = _open_directory(name, directory)
            os.close(directory)
            directory = inner
        return directory
    except BaseException:
        os.close(directory)
        raise


def _remove_expected_file(directory, leaf, expected):
    # Capture the name before inspecting it. Replacements at the original
    # name remain outside the private directory and survive this removal.
    holding = ".elastos-remove-" + secrets.token_hex(16)
    os.mkdir(holding, mode=0o700, dir_fd=directory)
    captured = _open_directory(holding, directory)
    empty = True
    try:
        try:
            os.rename(leaf, "entry", src_dir_fd=directory, dst_dir_fd=captured)
        except FileNotFoundError:
            return False
        empty = False
        found = os.lstat("entry", dir_fd=captured)
        identity = (found.st_dev, found.st_ino)
        if identity != tuple(expected) or stat.S_ISDIR(found.st_mode):
            # Restore a file/link only if the original name is still empty.
            # A conflict retains the captured file for explicit repair.
            try:
                os.link("entry", leaf, src_dir_fd=captured, dst_dir_fd=directory,
                        follow_symlinks=False)
                os.unlink("entry", dir_fd=captured)
                empty = True
            except OSError as error:
                raise UnsafeRemoval(f"{leaf} changed; its captured replacement is kept in {holding}/entry") from error
            raise UnsafeRemoval(f"{leaf} is no longer the recorded file; it was kept")
        os.unlink("entry", dir_fd=captured)
        empty = True
        return True
    finally:
        os.close(captured)
        if empty:
            os.rmdir(holding, dir_fd=directory)


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
        if not stat.S_ISDIR(found.st_mode) and expected is not None:
            return _remove_expected_file(directory, leaf, expected)
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
