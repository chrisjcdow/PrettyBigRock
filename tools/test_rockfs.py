#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Christopher Cameron-Dow
"""Tests for tools/rockfs.py: random operations checked against a model, extent trees,
full disks and simulated power cuts. Usage: test_rockfs.py [workdir]"""
import os
import random
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import rockfs  # noqa: E402

BLOCK = rockfs.BLOCK


def content(seed, size):
    r = random.Random(repr(seed))
    return r.randbytes(size)


def assert_clean(vol, what):
    problems = vol.check()
    assert not problems, '%s: %s' % (what, problems[:5])


def verify(vol, model):
    for path, data in model.items():
        if data is None:
            ino = vol.read_inode(vol.resolve(path))
            assert ino.type == rockfs.T_DIR, path
        else:
            got = vol.read_file(vol.read_inode(vol.resolve(path)))
            assert got == data, 'contents differ for %s (%d vs %d bytes)' % (path, len(got), len(data))


def random_ops(img, rounds, seed):
    r = random.Random(seed)
    vol = rockfs.Volume(img)
    model = {}
    folders = ['']
    for i in range(rounds):
        op = r.random()
        try:
            if op < 0.35 or not [p for p, d in model.items() if d is not None]:
                parent = r.choice(folders)
                path = '%s/File %d.txt' % (parent, r.randrange(60))
                size = r.choice([0, 1, 100, 4095, 4096, 4097, 9000, 30000, 70000])
                data = content((seed, i), size)
                if path in model and model[path] is None:
                    continue
                vol.put(path, data)
                model[path] = data
            elif op < 0.55:
                path = r.choice([p for p, d in model.items() if d is not None])
                old = model[path]
                off = r.randrange(len(old) + 1)
                patch = content((seed, i, 'p'), r.choice([1, 50, 5000]))
                new = old[:off] + patch + old[off + len(patch):]
                ino = vol.read_inode(vol.resolve(path))
                vol.write_range(vol.resolve(path), ino, off, patch, max(len(old), off + len(patch)))
                model[path] = new
            elif op < 0.7:
                path = r.choice([p for p, d in model.items() if d is not None])
                vol.remove(path)
                del model[path]
            elif op < 0.8 and len(folders) < 12:
                path = '%s/Folder %d' % (r.choice(folders), r.randrange(30))
                if path in model:
                    continue
                vol.create(path, rockfs.T_DIR)
                model[path] = None
                folders.append(path)
            elif op < 0.9:
                files = [p for p, d in model.items() if d is not None]
                src = r.choice(files)
                dst = '%s/Moved %d.TXT' % (r.choice(folders), r.randrange(40))
                if any(k.lower() == dst.lower() for k in model):
                    continue
                vol.rename(src, dst)
                model[dst] = model.pop(src)
            else:
                # case-insensitive lookups find the same file
                files = [p for p, d in model.items() if d is not None]
                path = r.choice(files)
                assert vol.resolve(path.upper()) == vol.resolve(path)
        except rockfs.RockFSError as e:
            if 'Not enough space' not in str(e):
                raise
        if i % 50 == 49:
            assert_clean(vol, 'after %d operations' % (i + 1))
            verify(vol, model)
    vol.close()
    vol = rockfs.Volume(img)
    verify(vol, model)
    assert_clean(vol, 'after reopening')
    vol.close()
    return model


def test_fragmentation(img):
    """Many small files, then delete every other one, so a big file needs many extents."""
    rockfs.mkfs(img, 8 * 1024 * 1024)
    vol = rockfs.Volume(img)
    for i in range(900):
        vol.put('/s%d' % i, content(i, BLOCK))
    for i in range(0, 900, 2):
        vol.remove('/s%d' % i)
    big = content('big', 450 * BLOCK)
    vol.put('/big.bin', big)
    ino = vol.read_inode(vol.resolve('/big.bin'))
    exts, tree = vol.load_extents(ino)
    assert len(exts) > rockfs.IN_INODE * rockfs.PER_XBLOCK // 4 or ino.depth >= 1, (len(exts), ino.depth)
    assert ino.depth >= 1 and tree, 'expected an extent tree'
    assert vol.read_file(ino) == big
    # edit the middle: only that part should get new blocks
    vol.write_range(vol.resolve('/big.bin'), ino, 100 * BLOCK + 7, b'hello', ino.size)
    big = big[:100 * BLOCK + 7] + b'hello' + big[100 * BLOCK + 12:]
    assert vol.read_file(vol.read_inode(vol.resolve('/big.bin'))) == big
    assert_clean(vol, 'fragmentation')
    vol.close()
    print('  extent tree: %d extents, depth %d' % (len(exts), ino.depth))


def test_full_disk(img):
    rockfs.mkfs(img, 2 * 1024 * 1024)
    vol = rockfs.Volume(img)
    vol.put('/keep.txt', b'precious')
    n = 0
    try:
        while True:
            vol.put('/fill%d' % n, content(n, 20 * BLOCK))
            n += 1
    except rockfs.RockFSError as e:
        assert 'Not enough space' in str(e), e
    before = vol.sb['free_blocks']
    # replacing a file with something bigger than the free space fails and changes nothing
    try:
        vol.put('/keep.txt', content('x', (before + 5) * BLOCK))
        raise AssertionError('expected Not enough space')
    except rockfs.RockFSError as e:
        assert 'Not enough space' in str(e)
    assert vol.read_file(vol.read_inode(vol.resolve('/keep.txt'))) == b'precious'
    assert vol.sb['free_blocks'] == before
    # deleting still works on a full disk, using the reserve
    vol.remove('/fill0')
    assert_clean(vol, 'full disk')
    vol.close()
    print('  full disk: %d files fitted, reserve kept, delete worked' % n)


def test_power_cut(img, seed):
    """Stop all writes after a random number, as if the power went off, then check."""
    r = random.Random(seed)
    rockfs.mkfs(img, 4 * 1024 * 1024)
    vol = rockfs.Volume(img)
    vol.put('/a.txt', content('a', 9000))
    vol.create('/docs', rockfs.T_DIR)
    vol.put('/docs/b.txt', content('b', 20000))
    vol.close()
    good = {'/a.txt': content('a', 9000), '/docs/b.txt': content('b', 20000)}
    new = {'/a.txt': content('a2', 15000), '/docs/b.txt': content('b2', 3000)}

    class PowerCut(Exception):
        pass
    vol = rockfs.Volume(img)
    budget = [r.randrange(1, 40)]
    real = vol.write_block

    def flaky(n, data, offset=0):
        if budget[0] <= 0:
            raise PowerCut()
        budget[0] -= 1
        # a write that's cut short leaves the sector it was writing half done
        if budget[0] == 0 and len(data) > rockfs.SECTOR and r.random() < 0.5:
            real(n, data[:rockfs.SECTOR], offset)
            raise PowerCut()
        real(n, data, offset)
    vol.write_block = flaky
    try:
        vol.put('/a.txt', new['/a.txt'])
        vol.put('/docs/b.txt', new['/docs/b.txt'])
        vol.put('/c.txt', b'new file')
        vol.remove('/docs/b.txt')
    except PowerCut:
        pass
    vol.f.close()
    vol = rockfs.Volume(img)
    problems = vol.check()
    for p in problems:  # only lost space is allowed, never damage
        assert 'belong' in p and 'nothing' in p or 'belong to no folder' in p, problems
    for path in good:
        try:
            got = vol.read_file(vol.read_inode(vol.resolve(path)))
        except rockfs.RockFSError:
            continue  # it was deleted
        assert got in (good[path], new[path]), 'half-written file %s' % path
    vol.check(repair=True)
    assert_clean(vol, 'after repair')
    vol.close()
    return len(problems)


def main():
    work = sys.argv[1] if len(sys.argv) > 1 else tempfile.mkdtemp()
    img = os.path.join(work, 'test.img')
    print('random operations')
    for seed in range(3):
        rockfs.mkfs(img, 12 * 1024 * 1024)
        model = random_ops(img, 600, seed)
        print('  seed %d: %d items survived' % (seed, len(model)))
    print('extent trees')
    test_fragmentation(img)
    print('full disk')
    test_full_disk(img)
    print('power cuts')
    leaks = sum(test_power_cut(img, s) > 0 for s in range(60))
    print('  60 power cuts: no damage; %d left lost space for check to reclaim' % leaks)
    print('mkfs variants')
    for whole in (False, True):
        if os.path.exists(img):
            os.remove(img)
        rockfs.mkfs(img, 2 * 1024 * 1024, whole_disk=whole)
        vol = rockfs.Volume(img)
        vol.put('/x', b'y')
        assert_clean(vol, 'mkfs whole=%s' % whole)
        vol.close()
    print('all tests passed')


if __name__ == '__main__':
    main()
