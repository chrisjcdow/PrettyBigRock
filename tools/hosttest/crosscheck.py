#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Christopher Cameron-Dow
"""Checks PrettyBigRock's C RockFS against the independent Python one.
Usage: crosscheck.py RFS_HOST WORKDIR"""
import os
import random
import subprocess
import sys
import zlib

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), '..'))
import rockfs  # noqa: E402

host, work = sys.argv[1], sys.argv[2]
img = os.path.join(work, 'x.img')
man = os.path.join(work, 'manifest.txt')


def run(*args, ok=(0,)):
    p = subprocess.run([host, img] + list(args), capture_output=True, text=True)
    if p.returncode not in ok:
        print(p.stdout, p.stderr)
        raise SystemExit('rfs_host %s failed (%d)' % (' '.join(args), p.returncode))
    return p


def blank(size):
    with open(img, 'wb') as f:
        f.truncate(size)


def py_verify(allow_leaks=False):
    vol = rockfs.Volume(img, writable=False)
    problems = vol.check()
    bad = [p for p in problems if not ('to nothing' in p or 'belong to no folder' in p)] if allow_leaks else problems
    assert not bad, bad[:5]
    n = 0
    for line in open(man):
        kind, length, crc, path = line.rstrip('\n').split(' ', 3)
        ino = vol.read_inode(vol.resolve(path))
        if kind == 'D':
            assert ino.type == rockfs.T_DIR, path
        else:
            data = vol.read_file(ino)
            assert len(data) == int(length) and '%08x' % zlib.crc32(data) == crc, path
        n += 1
    vol.close(clean=False)
    return n, len(problems)


print('1. C writes, Python reads and checks')
for seed in range(1, 6):
    blank(24 * 1024 * 1024)
    run('format')
    run('stress', str(seed), '700', man)
    n, _ = py_verify()
    print('   seed %d: %d items, Python check clean, contents match' % (seed, n))

print('2. Python writes, C reads; C writes, Python reads')
blank(16 * 1024 * 1024)
rockfs.mkfs(img, 16 * 1024 * 1024)
vol = rockfs.Volume(img)
r = random.Random(7)
expect = {}
vol.create('/Documents', rockfs.T_DIR)
for i in range(60):
    path = '/Documents/Note %d.txt' % i if i % 2 else '/file-%d.bin' % i
    data = r.randbytes(r.choice([0, 10, 5000, 40000, 300000]))
    vol.put(path, data)
    expect[path] = data
for i in range(0, 60, 4):  # fragment, then a big file with an extent tree
    p = '/Documents/Note %d.txt' % i if i % 2 else '/file-%d.bin' % i
    vol.remove(p)
    expect.pop(p)
for i in range(300):
    vol.put('/frag%d' % i, r.randbytes(4096))
for i in range(0, 300, 2):
    vol.remove('/frag%d' % i)
for i in range(1, 300, 2):
    expect['/frag%d' % i] = vol.read_file(vol.read_inode(vol.resolve('/frag%d' % i)))
big = r.randbytes(1500000)
vol.put('/big.bin', big)
expect['/big.bin'] = big
print('   big.bin uses extent depth %d' % vol.read_inode(vol.resolve('/big.bin')).depth)
vol.close()
with open(man, 'w') as f:
    for p, d in expect.items():
        f.write('%d %08x %s\n' % (len(d), zlib.crc32(d), p))
print('   ' + run('verify', man).stdout.strip())
vol = rockfs.Volume(img, writable=False)
assert not vol.check(), vol.check()[:5]
assert vol.read_file(vol.read_inode(vol.resolve('/from prettybigrock/HELLO.TXT'))) == b'Hello from the C side\n'
vol.close(clean=False)
print('   Python sees the C changes; check clean')

print('3. power cuts in the C code')
leaky = 0
for trial in range(40):
    blank(8 * 1024 * 1024)
    run('format')
    run('stress', str(100 + trial), '200', man)  # a baseline
    cut = random.Random(trial).randrange(5, 3000)
    run('stress', str(200 + trial), '400', os.path.join(work, 'ignored.txt'), str(cut))
    vol = rockfs.Volume(img, writable=False)
    problems = vol.check()
    bad = [p for p in problems if not ('to nothing' in p or 'belong to no folder' in p)]
    assert not bad, (trial, cut, bad[:5])
    leaky += bool(problems)
    vol.close(clean=False)
    run('check', 'repair')
    vol = rockfs.Volume(img, writable=False)
    assert not vol.check(), (trial, vol.check()[:5])
    vol.close(clean=False)
print('   40 power cuts: never damaged; %d needed space reclaimed; C repair left Python check clean' % leaky)
print('all cross-checks passed')
