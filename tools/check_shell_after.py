#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Christopher Cameron-Dow
"""Checks the system volume after the shell test (docs/design/0018-supervision-shell-and-m5-tests.md,
section 7, test 8).

Usage: check_shell_after.py IMAGE

IMAGE is the GPT test disk (xtask/src/testdisks.rs), whose RockFS volume `Home` is the system
volume. The test runner typed commands into the shell (xtask/src/qemu.rs, SHELL_COMMANDS):
they made a folder in chris's home, wrote a file there and renamed it, wrote another and
removed it, and finally `shutdown` had the storage service save every volume. This checks,
with the Python tool, that:
- the volume was shut down cleanly and is undamaged;
- the shell's changes are on the disk: the renamed file holds exactly what was written, and
  neither the file's old name nor the removed file is there.

Keep FOLDER, the names and GREETING the same as SHELL_COMMANDS.
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import rockfs  # noqa: E402

FOLDER = '/Users/chris/Shelltest'
GREETING = b'Hello from the shell\n'


def fail(message):
    raise SystemExit('check_shell_after: ' + message)


def exists(vol, path):
    try:
        vol.resolve(path)
        return True
    except rockfs.RockFSError:
        return False


def main(argv):
    if len(argv) != 2:
        print(__doc__.strip())
        return 2
    vol = rockfs.Volume(argv[1], writable=False)
    try:
        if vol.sb['state'] != 0:
            fail('the volume was not shut down cleanly')
        problems = vol.check()
        if problems:
            fail('the Python tool finds: %s' % problems[:5])
        if not exists(vol, FOLDER + '/greeting.txt'):
            fail('%s/greeting.txt is not there' % FOLDER)
        data = vol.read_file(vol.read_inode(vol.resolve(FOLDER + '/greeting.txt')))
        if data != GREETING:
            fail('greeting.txt holds %r, not %r' % (data, GREETING))
        for gone in ('hello.txt', 'gone.txt'):
            if exists(vol, FOLDER + '/' + gone):
                fail('%s/%s is still there' % (FOLDER, gone))
    finally:
        vol.close(clean=False)
    print('check_shell_after: the system volume is clean, with the shell\'s changes')
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv))
