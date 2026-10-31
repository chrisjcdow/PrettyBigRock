#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Christopher Cameron-Dow
"""An independent reader for PrettyBigRock's start-up archive, written from
docs/design/0009-programs-and-granite.md section 1 alone, for cross-checking lib/archive.

Usage:
  startarc.py list ARCHIVE           check the archive and list its files
  startarc.py extract ARCHIVE DIR    check it and write its files into DIR
"""
import os
import struct
import sys
import zlib

MAGIC = b"PBRARCHV"
PAGE = 4096


class BadArchive(Exception):
    pass


def parse(data):
    """Checks every rule in the specification; returns [(name, offset, bytes)]."""
    if len(data) < 64:
        raise BadArchive("shorter than a header")
    if data[:8] != MAGIC:
        raise BadArchive("wrong magic")
    major, minor, count, total, crc = struct.unpack_from("<HHIQI", data, 8)
    if major != 1:
        raise BadArchive(f"unsupported version {major}")
    if total == 0 or total % PAGE:
        raise BadArchive("size isn't a whole number of pages")
    if total > len(data):
        raise BadArchive("cut short")
    data = data[:total]
    if zlib.crc32(data[:24] + b"\0\0\0\0" + data[28:]) != crc:
        raise BadArchive("checksum doesn't match")
    if minor == 0 and any(data[28:64]):
        raise BadArchive("reserved header bytes aren't zero")
    table_end = 64 + 64 * count
    if table_end > total:
        raise BadArchive("entry table runs past the end")
    files = []
    names = set()
    gap_from = table_end
    for i in range(count):
        e = 64 + 64 * i
        raw = data[e:e + 48]
        n = raw.find(b"\0")
        n = 48 if n < 0 else n
        if n == 0 or n > 47 or any(raw[n:]):
            raise BadArchive(f"file {i}: bad name")
        try:
            name = raw[:n].decode("utf-8")
        except UnicodeDecodeError:
            raise BadArchive(f"file {i}: name isn't UTF-8")
        if name in names:
            raise BadArchive(f"file {i}: name used twice")
        names.add(name)
        offset, size = struct.unpack_from("<QQ", data, e + 48)
        if offset % PAGE or offset < gap_from or offset + size > total:
            raise BadArchive(f"file {i}: in the wrong place")
        if any(data[gap_from:offset]):
            raise BadArchive("space between files isn't empty")
        files.append((name, offset, data[offset:offset + size]))
        gap_from = offset + size
    if any(data[gap_from:]):
        raise BadArchive("space after the last file isn't empty")
    return files


def main(argv):
    if len(argv) < 3 or argv[1] not in ("list", "extract"):
        print(__doc__.strip(), file=sys.stderr)
        return 2
    with open(argv[2], "rb") as f:
        data = f.read()
    try:
        files = parse(data)
    except BadArchive as e:
        print(f"{argv[2]}: {e}", file=sys.stderr)
        return 1
    for name, offset, body in files:
        if argv[1] == "list":
            print(f"{offset:#010x} {len(body):>10}  {name}")
        else:
            out = os.path.join(argv[3], *name.split("/"))
            os.makedirs(os.path.dirname(out), exist_ok=True)
            with open(out, "wb") as f:
                f.write(body)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
