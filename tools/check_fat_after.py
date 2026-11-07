#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Christopher Cameron-Dow
"""Checks the FAT32 test volume after PrettyBigRock 2 has written to it
(docs/design/0016-filesystems.md, section 5), with Linux's own FAT tools.

Usage: check_fat_after.py IMAGE START SECTORS

IMAGE is the MBR test disk (xtask/src/testdisks.rs); the volume is SECTORS sectors from sector
START. The root program's self-test (services/root/src/fattest.rs) wrote to it and the storage
service then shut it down. This checks that:
- `fsck.fat -n` finds nothing wrong, and the volume was marked clean;
- `mtools` reads back exactly what the self-test wrote.

Keep HELLO, UNICODE and BIG the same as fattest.rs.
"""
import os
import shutil
import subprocess
import sys
import tempfile

README = b"A FAT32 test volume for PrettyBigRock's storage tests.\n"
HELLO = b'Hello, FAT, from PrettyBigRock 2.\n'
UNICODE = 'Ünïcödé — a long name for a file.txt'
BIG = bytes(i * 13 % 253 for i in range(200_000))
FOLDER = 'From PrettyBigRock 2'


def fail(message):
    raise SystemExit('check_fat_after: ' + message)


def main(argv):
    if len(argv) != 4:
        print(__doc__.strip())
        return 2
    image, start, sectors = argv[1], int(argv[2]), int(argv[3])
    for tool in ('fsck.fat', 'mtype', 'mdir'):
        if shutil.which(tool) is None:
            print('check_fat_after: skipped: %s is not installed (dosfstools and mtools)' % tool)
            return 0
    with tempfile.TemporaryDirectory() as tmp:
        part = os.path.join(tmp, 'part.img')
        with open(image, 'rb') as f:
            f.seek(start * 512)
            data = f.read(sectors * 512)
        with open(part, 'wb') as f:
            f.write(data)

        r = subprocess.run(['fsck.fat', '-n', part], capture_output=True, text=True)
        out = r.stdout + r.stderr
        if r.returncode != 0 or 'Dirty bit' in out or 'differences' in out.lower():
            fail('fsck.fat -n finds:\n' + out)

        env = dict(os.environ, MTOOLS_SKIP_CHECK='1', LC_ALL='C.UTF-8')

        def mtype(path):
            r = subprocess.run(['mtype', '-i', part, '::' + path], capture_output=True, env=env)
            if r.returncode != 0:
                fail('mtools cannot read %s: %s' % (path, r.stderr.decode(errors='replace')))
            return r.stdout

        def names(path):
            r = subprocess.run(['mdir', '-i', part, '-b', '::' + path], capture_output=True,
                               env=env, text=True)
            if r.returncode != 0:
                fail('mtools cannot list %s: %s' % (path, r.stderr))
            return sorted(os.path.basename(line.rstrip('/')) for line in r.stdout.splitlines()
                          if line.strip())

        if mtype('/README.TXT') != README:
            fail('README.TXT changed')
        if mtype('/%s/Hello again.txt' % FOLDER) != HELLO:
            fail('Hello again.txt reads back wrongly')
        if mtype('/%s/%s' % (FOLDER, UNICODE)) != b'':
            fail('the file with a non-ASCII name reads back wrongly')
        if mtype('/big.bin') != BIG:
            fail('big.bin reads back wrongly')
        want = sorted(['Hello again.txt', UNICODE])
        if names('/' + FOLDER) != want:
            fail('the folder lists as %s, not %s' % (names('/' + FOLDER), want))
        want = sorted(['README.TXT', FOLDER, 'big.bin'])
        if names('/') != want:
            fail('the root lists as %s, not %s' % (names('/'), want))
    print('check_fat_after: fsck.fat finds it clean, and mtools reads back what was written')
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv))
