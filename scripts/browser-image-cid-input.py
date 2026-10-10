#!/usr/bin/env python3
"""Validate a build-time immutable image selector before native compilation."""
import base64
import re
import sys


def validate(cid, checksum, size):
    if not re.fullmatch(r"[0-9a-f]{64}", checksum):
        raise ValueError("Browser image SHA-256 requires 64 lowercase hex digits")
    if not re.fullmatch(r"[1-9][0-9]*", size) or not 0 < int(size) <= 16 * 1024**3:
        raise ValueError("Browser image size requires 1..16 GiB")
    if cid.startswith("Qm"):
        alphabet = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
        number = 0
        for char in cid:
            number = number * 58 + alphabet.index(char)
        raw = number.to_bytes((number.bit_length() + 7) // 8, "big")
        if len(cid) != 46 or len(raw) != 34 or raw[:2] != b"\x12\x20":
            raise ValueError("Browser image CIDv0 requires a canonical SHA-256 multihash")
    else:
        if not re.fullmatch(r"b[a-z2-7]+", cid):
            raise ValueError("Browser image CID requires canonical CIDv0 or base32 CIDv1")
        raw = base64.b32decode(cid[1:].upper() + "=" * (-len(cid[1:]) % 8))
        if (len(raw) != 36 or raw[:4] not in (b"\x01\x70\x12\x20", b"\x01\x55\x12\x20")
                or "b" + base64.b32encode(raw).decode().lower().rstrip("=") != cid):
            raise ValueError("Browser image CIDv1 requires a canonical raw or UnixFS SHA-256 CID")


if __name__ == "__main__":
    try:
        if len(sys.argv) != 4:
            raise ValueError("Supply image CID, SHA-256 and size")
        validate(*sys.argv[1:])
    except (ValueError, TypeError) as error:
        sys.exit(str(error))
