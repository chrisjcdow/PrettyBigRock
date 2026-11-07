#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Christopher Cameron-Dow
"""Checks the disk 1.3 made after PrettyBigRock 2 has used it (docs/design/0016-filesystems.md,
section 4, layer 4).

Usage: check_classic_after.py IMAGE MANIFEST [RFS_HOST]

The boot tests attach a fresh copy of tests/data/classic-1.3.img.gz; the root program's
self-test (services/root/src/classictest.rs) checks it, changes it as CHANGES below says, and
shuts the storage service down, which marks it clean. This then checks, with the Python tool
and (when RFS_HOST, 1.3's C code built for the host, is given) with 1.3's own code, that:
- the volume was shut down cleanly and is undamaged;
- it holds exactly what MANIFEST (as 1.3 left it) lists, with CHANGES made, byte for byte.

Keep CHANGES and the contents below the same as classictest.rs.
"""
import os
import subprocess
import sys
import zlib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import rockfs  # noqa: E402

HELLO = b'Hello from PrettyBigRock 2.\n'
PS = b'PS from PrettyBigRock 2.\n'
BIG = bytes(i * 7 % 251 for i in range(300_000))
NEW_FOLDER = '/From PrettyBigRock 2'
CHANGES = {
    'new': {NEW_FOLDER + '/hello.txt': HELLO, NEW_FOLDER + '/big.bin': BIG},
    'append': ('/Documents/letter.txt', PS),
    'renames': [('/Projects/Rock/moved.txt', '/Projects/Rock/renamed by 2.txt'),
                ('/Projects/Rock/Deep/notes.txt', '/Documents/notes.txt')],
    'remove': '/Fill/f01',
}


def fail(message):
    raise SystemExit('check_classic_after: ' + message)


def expected(manifest):
    """The manifest as it should be now: {path: (folder, size, crc)}."""
    want = {}
    for line in open(manifest, encoding='utf-8'):
        kind, size, crc, path = line.rstrip('\n').split(' ', 3)
        want[path] = (kind == 'D', int(size), int(crc, 16))
    want[NEW_FOLDER] = (True, 0, 0)
    for path, data in CHANGES['new'].items():
        want[path] = (False, len(data), zlib.crc32(data))
    path, extra = CHANGES['append']
    _, size, crc = want[path]
    want[path] = (False, size + len(extra), zlib.crc32(extra, crc))  # CRC-32 carries on
    for old, new in CHANGES['renames']:
        want[new] = want.pop(old)
    del want[CHANGES['remove']]
    return want


def main(argv):
    if len(argv) < 3:
        print(__doc__.strip())
        return 2
    image, manifest = argv[1], argv[2]
    host = argv[3] if len(argv) > 3 else None
    want = expected(manifest)

    vol = rockfs.Volume(image, writable=False)
    try:
        if vol.sb['state'] != 0:
            fail('the volume was not shut down cleanly')
        problems = vol.check()
        if problems:
            fail('the Python tool finds: %s' % problems[:5])
        found = {}

        def walk(path, n):
            for _, _, inum, etype, name in vol.dir_entries(n):
                p = path.rstrip('/') + '/' + name
                if etype == rockfs.T_DIR:
                    found[p] = (True, 0, 0)
                    walk(p, inum)
                else:
                    d = vol.read_file(vol.read_inode(inum))
                    found[p] = (False, len(d), zlib.crc32(d))
        walk('/', vol.resolve('/'))
    finally:
        vol.close(clean=False)
    if found != want:
        missing = sorted(set(want) - set(found))
        extra = sorted(set(found) - set(want))
        differ = sorted(p for p in set(want) & set(found) if want[p] != found[p])
        fail('missing %s; unexpected %s; different %s' % (missing, extra, differ))
    print('check_classic_after: the Python tool finds it clean, with exactly the %d items expected'
          % len(found))

    if host:
        # 1.3's code: its check, then every file read back. It mounts the volume, so it runs
        # last (mounting changes the mount count and time).
        files = os.path.join(os.path.dirname(os.path.abspath(image)), 'classic-after.manifest')
        with open(files, 'w', encoding='utf-8') as f:
            for p, (folder, size, crc) in sorted(want.items()):
                if not folder:
                    f.write('%d %08x %s\n' % (size, crc, p))
        r = subprocess.run([host, image, 'check'], capture_output=True, text=True)
        if r.returncode != 0 or not r.stdout.startswith('0 problem'):
            fail('1.3\'s check finds:\n%s%s' % (r.stdout, r.stderr))
        r = subprocess.run([host, image, 'verify', files], capture_output=True, text=True)
        if r.returncode != 0:
            fail('1.3 reads the files back wrongly:\n%s%s' % (r.stdout, r.stderr))
        print('check_classic_after: 1.3\'s code finds it clean and reads every file back')
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv))
