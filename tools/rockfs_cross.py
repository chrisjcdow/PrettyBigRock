#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Christopher Cameron-Dow
"""Checks PrettyBigRock's Rust RockFS (lib/rockfs, through tools/rockfs-rs) against the other two
implementations of docs/ROCKFS.md: the independent Python tool (tools/rockfs.py) and the C code
the classic system uses (classic/tools/hosttest/rfs_host).

Usage: rockfs_cross.py ROCKFS_RS WORKDIR [RFS_HOST]

Layer 2, Rust and Python, always:
  every round makes an image, then lets the two take turns on it. Rust writes (a random tree
  made by `stress`), and Python must find the volume clean and every file as listed in the
  manifest; Python writes (random folders, files of many sizes, a file with an extent tree),
  and Rust must find it clean and verify the same; and so on.

The disk 1.3 made, always (tests/data/classic-1.3.img.gz, made by `cargo xtask classic-disk`):
  Rust must find it clean and holding exactly what its manifest lists, then add to it (with
  `stress`), leaving something Python finds clean and exactly as Rust's manifest says, and,
  when RFS_HOST is given, that the C code reads back and finds clean too.

Layer 3, Rust and C, only when RFS_HOST is given:
  the C code formats and writes, and Rust must read it, check it clean, add to it, and leave
  something the C code and Python both find clean. Images cut off in the middle of a write by
  the C code must be mounted by Rust (which repairs them) with nothing lost beyond leaked
  space, leaving a volume that all three find clean.
"""
import os
import random
import gzip
import shutil
import subprocess
import sys
import zlib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import rockfs  # noqa: E402

LEAKS = ('to nothing', 'belong to no folder')  # what a cut-off write may leave (Python's words)


def fail(message):
    raise SystemExit('rockfs_cross: ' + message)


class Tools:
    """The programs, an image path and a manifest path to work with."""

    def __init__(self, rs, work, host=None):
        self.rs, self.host = rs, host
        self.work = work
        self.img = os.path.join(work, 'x.img')
        self.man = os.path.join(work, 'manifest.txt')

    def rs_run(self, *args, ok=(0,)):
        p = subprocess.run([self.rs, self.img] + list(args), capture_output=True, text=True)
        if p.returncode not in ok:
            fail('rockfs-rs %s failed (%d):\n%s%s' % (' '.join(args), p.returncode, p.stdout, p.stderr))
        return p

    def host_run(self, *args, ok=(0,)):
        p = subprocess.run([self.host, self.img] + list(args), capture_output=True, text=True)
        if p.returncode not in ok:
            fail('rfs_host %s failed (%d):\n%s%s' % (' '.join(args), p.returncode, p.stdout, p.stderr))
        return p

    def blank(self, size):
        with open(self.img, 'wb') as f:
            f.truncate(size)


# ------------------------------------------------------------------ Python's view of a volume

def py_walk(vol):
    """Every file and folder as (path, is_folder, contents or None), from the Python tool."""
    out = []

    def go(n, base):
        for _, _, inum, etype, name in vol.dir_entries(n):
            path = base + '/' + name
            ino = vol.read_inode(inum)
            if etype == rockfs.T_DIR:
                out.append((path, True, None))
                go(inum, path)
            else:
                out.append((path, False, vol.read_file(ino)))
    go(vol.sb['root_inode'], '')
    return out


def manifest_text(tree):
    return ''.join('%s %d %08x %s\n' % ('D' if d else 'F', 0 if d else len(c), 0 if d else zlib.crc32(c), p)
                   for p, d, c in sorted(tree))


def read_manifest(path):
    items = {}
    for line in open(path, encoding='utf-8'):
        kind, length, crc, name = line.rstrip('\n').split(' ', 3)
        items[name] = (kind == 'D', int(length), crc)
    return items


def py_verify(img, man, allow_leaks=False):
    """Python checks the volume and finds exactly the manifest's files, with the same contents."""
    vol = rockfs.Volume(img, writable=False)
    try:
        problems = vol.check()
        bad = [p for p in problems if not any(w in p for w in LEAKS)] if allow_leaks else problems
        if bad:
            fail('Python check found: %s' % bad[:5])
        want = read_manifest(man)
        tree = py_walk(vol)
        if len(tree) != len(want):
            fail('Python sees %d items, the manifest lists %d' % (len(tree), len(want)))
        for path, folder, data in tree:
            w = want.get(path)
            if w is None:
                fail('Python sees %s, which the manifest does not list' % path)
            if w[0] != folder:
                fail('%s: folder or file differs' % path)
            if not folder and (len(data) != w[1] or '%08x' % zlib.crc32(data) != w[2]):
                fail('%s: contents differ' % path)
        return len(tree), len(problems)
    finally:
        vol.close(clean=False)


def py_write_tree(img, rng, tag):
    """Python adds folders, files of many sizes and a fragmented big file; returns the depth of
    the biggest extent tree it made."""
    vol = rockfs.Volume(img)
    sizes = [0, 1, 100, 4095, 4096, 4097, 9000, 70000, 200000]
    folders = ['']
    for i in range(10):
        parent = rng.choice(folders)
        if parent.count('/') >= 4:
            continue
        path = '%s/Py Folder %s%d' % (parent, tag, i)
        vol.create(path, rockfs.T_DIR)
        folders.append(path)
    for i in range(40):
        name = rng.choice(['Py File %s%d.txt', 'PY FILE %s%d.TXT', 'py-file-%s%d.bin'])
        vol.put('%s/%s' % (rng.choice(folders), name % (tag, i)), rng.randbytes(rng.choice(sizes)))
    # Fragment the free space, then write a file that needs a tree of extent blocks.
    for i in range(160):
        vol.put('/Py Frag %s%d' % (tag, i), rng.randbytes(4096))
    for i in range(0, 160, 2):
        vol.remove('/Py Frag %s%d' % (tag, i))
    vol.put('/Py Big %s.bin' % tag, rng.randbytes(rng.randrange(900_000, 1_300_000)))
    depth = vol.read_inode(vol.resolve('/Py Big %s.bin' % tag)).depth
    # Rename a few things, including a folder with contents.
    vol.rename('/Py Folder %s0' % tag, '/Py Moved %s' % tag)
    vol.close()
    return depth


# ------------------------------------------------------------------ layer 2

def layer2(t):
    rounds = 0
    deepest = 0
    items = 0
    for seed in range(1, 5):
        rng = random.Random(seed)
        size = rng.choice([32, 40, 48]) * 1024 * 1024
        t.blank(size)
        t.rs_run('mkfs', '--size', str(size), *(['--whole-disk'] if seed % 2 == 0 else []))
        for turn in range(2):
            t.rs_run('stress', str(seed * 100 + turn), '300', t.man)
            items, _ = py_verify(t.img, t.man)              # Rust wrote, Python reads
            deepest = max(deepest, py_write_tree(t.img, rng, 's%dt%d' % (seed, turn)))
            vol = rockfs.Volume(t.img, writable=False)
            tree = py_walk(vol)
            vol.close(clean=False)
            with open(t.man, 'w', encoding='utf-8') as f:
                f.write(manifest_text(tree))
            r = t.rs_run('check')                           # Python wrote, Rust checks
            if 'No problems found.' not in r.stdout:
                fail('rockfs-rs check after Python writes: ' + r.stdout)
            t.rs_run('verify', '--exact', t.man)
            rounds += 1
        # Python's own `mv` and `rm` on what Rust made, then Rust reads it.
        vol = rockfs.Volume(t.img)
        victims = [p for p, d, _ in py_walk(vol) if not d][:5]
        for p in victims:
            vol.remove(p)
        vol.close()
        vol = rockfs.Volume(t.img, writable=False)
        tree = py_walk(vol)
        vol.close(clean=False)
        with open(t.man, 'w', encoding='utf-8') as f:
            f.write(manifest_text(tree))
        t.rs_run('verify', '--exact', t.man)
        t.rs_run('check')
    if deepest < 1:
        fail('no file in layer 2 needed an extent tree')
    # The command line tools agree about the same image.
    py_ls = subprocess.run([sys.executable, os.path.join(os.path.dirname(os.path.abspath(__file__)), 'rockfs.py'),
                            t.img, 'ls', '-R'], capture_output=True, text=True)
    rs_ls = t.rs_run('ls', '-R')
    if py_ls.returncode or sorted(l.split()[0] for l in py_ls.stdout.splitlines()) != \
            sorted(l.split()[0] for l in rs_ls.stdout.splitlines()):
        fail('ls -R differs between rockfs.py and rockfs-rs')
    print('layer 2 (Rust and Python): %d alternating rounds on 4 images, last had %d items; '
          'deepest extent tree %d; all checks clean' % (rounds, items, deepest))


# ------------------------------------------------------------------ layer 3

def to_host_manifest(src, dst):
    """rfs_host's `verify` reads lines of `length crc path` for files only."""
    with open(dst, 'w', encoding='utf-8') as f:
        for p, (folder, length, crc) in sorted(read_manifest(src).items()):
            if not folder:
                f.write('%d %s %s\n' % (length, crc, p))


def layer3(t):
    # C writes, Rust reads and adds, C and Python read.
    for seed in range(1, 4):
        t.blank(24 * 1024 * 1024)
        t.host_run('format')
        t.host_run('stress', str(seed), '600', t.man)
        r = t.rs_run('check')
        if 'No problems found.' not in r.stdout:
            fail('rockfs-rs check of the C code\'s volume: ' + r.stdout)
        t.rs_run('verify', '--exact', t.man)
        py_verify(t.img, t.man)
        t.rs_run('stress', str(seed + 50), '400', t.man)
        host_man = os.path.join(t.work, 'host-manifest.txt')
        to_host_manifest(t.man, host_man)
        t.host_run('verify', host_man)
        h = t.host_run('check')
        if not h.stdout.startswith('0 problem'):
            fail('rfs_host check of the volume Rust wrote: ' + h.stdout)
        vol = rockfs.Volume(t.img, writable=False)
        left = vol.check()
        vol.close(clean=False)
        if left:
            fail('Python check after Rust and C: %s' % left[:5])
    print('layer 3 (Rust and C): 3 images written by C, extended by Rust, read by C and Python; all clean')
    # Power cuts in the C code: Rust must repair them.
    leaky = 0
    trials = 25
    for trial in range(trials):
        t.blank(8 * 1024 * 1024)
        t.host_run('format')
        t.host_run('stress', str(100 + trial), '200', t.man)
        cut = random.Random(trial).randrange(5, 3000)
        t.host_run('stress', str(200 + trial), '400', os.path.join(t.work, 'ignored.txt'), str(cut))
        vol = rockfs.Volume(t.img, writable=False)
        problems = vol.check()
        vol.close(clean=False)
        bad = [p for p in problems if not any(w in p for w in LEAKS)]
        if bad:
            fail('cut %d after %d writes: Python sees damage: %s' % (trial, cut, bad[:5]))
        before = open(t.img, 'rb').read()
        r = t.rs_run('check', ok=(0, 2))                    # looks only
        if open(t.img, 'rb').read() != before:
            fail('rockfs-rs check changed the image')
        for line in r.stdout.splitlines():
            if line.startswith('Problem:') and not line.endswith('(harmless)'):
                fail('cut %d after %d writes: Rust sees damage: %s' % (trial, cut, line))
        r = t.rs_run('check', '--repair')                   # mounts and repairs
        for line in r.stdout.splitlines():
            if not line.endswith('(harmless)') and line != 'No problems found.':
                fail('cut %d after %d writes: Rust repaired something that was not just leaked space: %s'
                     % (trial, cut, line))
        leaky += bool(problems)
        r = t.rs_run('check')
        if 'No problems found.' not in r.stdout:
            fail('cut %d: check after repair: %s' % (trial, r.stdout))
        vol = rockfs.Volume(t.img, writable=False)
        left = vol.check()
        vol.close(clean=False)
        if left:
            fail('cut %d: Python check after Rust repair: %s' % (trial, left[:5]))
        h = t.host_run('check')
        if not h.stdout.startswith('0 problem'):
            fail('cut %d: rfs_host check after Rust repair: %s' % (trial, h.stdout))
    print('layer 3 (power cuts in C): %d images, never damaged (%d had space to reclaim); '
          'Rust repaired them; Rust, Python and C check clean' % (trials, leaky))


# ------------------------------------------------------------------ the disk 1.3 made

CLASSIC = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'tests', 'data', 'classic-1.3')


def classic_disk(t):
    with gzip.open(CLASSIC + '.img.gz', 'rb') as src, open(t.img, 'wb') as dst:
        shutil.copyfileobj(src, dst)
    r = t.rs_run('check')
    if 'No problems found.' not in r.stdout:
        fail('the disk 1.3 made: rockfs-rs check: %s' % r.stdout)
    t.rs_run('verify', '--exact', CLASSIC + '.manifest')
    t.rs_run('stress', '1300', '150', t.man)
    n, _ = py_verify(t.img, t.man)
    if t.host:
        host_man = os.path.join(t.work, 'host-manifest.txt')
        to_host_manifest(t.man, host_man)
        t.host_run('verify', host_man)
        h = t.host_run('check')
        if not h.stdout.startswith('0 problem'):
            fail('the disk 1.3 made, after Rust wrote to it: rfs_host check: %s' % h.stdout)
    print('the disk 1.3 made: Rust reads it exactly as its manifest says, adds to it (now %d items), '
          'and Python%s read it back clean' % (n, ' and C' if t.host else ''))


def main(argv):
    if len(argv) < 3:
        print(__doc__.strip())
        return 1
    rs, work = os.path.abspath(argv[1]), argv[2]
    host = os.path.abspath(argv[3]) if len(argv) > 3 else None
    shutil.rmtree(work, ignore_errors=True)
    os.makedirs(work)
    t = Tools(rs, work, host)
    layer2(t)
    classic_disk(t)
    if host:
        layer3(t)
    print('rockfs_cross: all passed')
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv))
