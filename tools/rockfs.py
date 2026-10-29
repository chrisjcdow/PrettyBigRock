#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Christopher Cameron-Dow
"""rockfs: read and write RockFS disk images from Windows, Linux or macOS.

An independent implementation of docs/ROCKFS.md (version 1), using only the Python
standard library. It is also used to test PrettyBigRock's own implementation.

Usage:
  rockfs.py IMAGE mkfs [--size 32M] [--label Data] [--whole-disk]
  rockfs.py IMAGE info
  rockfs.py IMAGE ls [PATH] [-R]
  rockfs.py IMAGE cat PATH
  rockfs.py IMAGE get PATH [DEST]
  rockfs.py IMAGE put SOURCE PATH
  rockfs.py IMAGE mkdir PATH
  rockfs.py IMAGE rm PATH
  rockfs.py IMAGE mv FROM TO
  rockfs.py IMAGE check [--repair]
"""
import os
import struct
import sys
import time
import unicodedata
import uuid
import zlib

BLOCK = 4096
SECTOR = 512
MAGIC = b'ROCKFS!\0'
VERSION = 1
MBR_TYPE = 0x7F
GPT_TYPE = uuid.UUID('03971446-6A02-47DC-BB5F-6A7572E5247C').bytes_le
PART_START = 2048  # sectors (1 MiB)

SB_FMT = '<8sHHHHQQQQQQIIIIIIIIQQ16s64s320x256s252xI'
INODE_FMT = '<HHIQQQIHH24x168s20xI'
EXT_FMT = '<QQII'
IDX_FMT = '<QQ8x'
XB_HDR = '<4sHHII'
DE_HDR = '<IHHB7x'
assert struct.calcsize(SB_FMT) == 1024
assert struct.calcsize(INODE_FMT) == 256
assert struct.calcsize(EXT_FMT) == 24 and struct.calcsize(IDX_FMT) == 24
assert struct.calcsize(XB_HDR) == 16 and struct.calcsize(DE_HDR) == 16

T_FREE, T_FILE, T_DIR = 0, 1, 2
IN_INODE = 7
PER_XBLOCK = 170
INODES_PER_BLOCK = BLOCK // 256
TABLE_INO = -1  # the inode table's own inode, which lives in the superblock
FORBIDDEN = set('/\\:*?"<>|')


class RockFSError(Exception):
    pass


def crc(data):
    return zlib.crc32(data) & 0xFFFFFFFF


def now():
    return int(time.time())


def align8(n):
    return (n + 7) & ~7


def name_key(name):
    """Name rule 1: A-Z match a-z, everything else exactly."""
    return bytes((b + 32) if 65 <= b <= 90 else b for b in name.encode('utf-8'))


def check_name(name):
    name = unicodedata.normalize('NFC', name)
    if not name or len(name) > 255:
        raise RockFSError('Names must be 1 to 255 characters long')
    if name in ('.', '..'):
        raise RockFSError('A name can\'t be "." or ".."')
    for ch in name:
        if ord(ch) < 32 or ord(ch) == 127 or ch in FORBIDDEN:
            raise RockFSError('Names can\'t contain control characters or any of / \\ : * ? " < > |')
    if name[-1] in ' .':
        raise RockFSError('Names can\'t end with a space or a dot')
    return name


class Inode:
    __slots__ = ('type', 'flags', 'parent', 'size', 'created', 'modified', 'generation',
                 'count', 'depth', 'entries')

    def __init__(self):
        self.type = T_FREE
        self.flags = 0
        self.parent = 0
        self.size = 0
        self.created = 0
        self.modified = 0
        self.generation = 0
        self.count = 0
        self.depth = 0
        self.entries = []  # tuples (file_block, disk_block, length) or (file_block, child)

    @classmethod
    def unpack(cls, raw, verify=True):
        f = struct.unpack(INODE_FMT, raw)
        ino = cls()
        (ino.type, ino.flags, ino.parent, ino.size, ino.created, ino.modified,
         ino.generation, ino.count, ino.depth, ents, check) = f
        if verify and ino.type != T_FREE and check != crc(raw[:252]):
            raise RockFSError('inode checksum mismatch')
        if ino.count > IN_INODE:
            raise RockFSError('inode has too many entries')
        ino.entries = []
        for i in range(ino.count):
            e = ents[i * 24:(i + 1) * 24]
            if ino.depth == 0:
                fb, db, ln, _ = struct.unpack(EXT_FMT, e)
                ino.entries.append((fb, db, ln))
            else:
                fb, child = struct.unpack(IDX_FMT, e)
                ino.entries.append((fb, child))
        return ino

    def pack(self):
        ents = b''
        for e in self.entries:
            ents += struct.pack(EXT_FMT, e[0], e[1], e[2], 0) if self.depth == 0 else struct.pack(IDX_FMT, e[0], e[1])
        ents = ents.ljust(168, b'\0')
        raw = struct.pack(INODE_FMT, self.type, self.flags, self.parent, self.size, self.created,
                          self.modified, self.generation, len(self.entries), self.depth, ents, 0)
        return raw[:252] + struct.pack('<I', crc(raw[:252]))


def tree_blocks_needed(n):
    if n <= IN_INODE:
        return 0
    level = -(-n // PER_XBLOCK)
    total = level
    while level > IN_INODE:
        level = -(-level // PER_XBLOCK)
        total += level
    return total


def merge_extents(exts):
    exts = sorted(exts)
    out = []
    for fb, db, ln in exts:
        if out and out[-1][0] + out[-1][2] == fb and out[-1][1] + out[-1][2] == db and out[-1][2] + ln < 2**32:
            out[-1] = (out[-1][0], out[-1][1], out[-1][2] + ln)
        else:
            out.append((fb, db, ln))
    return out


class Volume:
    def __init__(self, path, writable=True):
        self.f = open(path, 'r+b' if writable else 'rb')
        self.writable = writable
        self.f.seek(0, 2)
        disk_size = self.f.tell()
        self.offset, self.length = self._find(disk_size)
        self.sb = None
        self._load_superblock()

    # ---------------------------------------------------------------- raw access
    def _read_at(self, pos, n):
        self.f.seek(pos)
        data = self.f.read(n)
        return data.ljust(n, b'\0')

    def _find(self, disk_size):
        s0 = self._read_at(0, SECTOR)
        if s0[:8] == MAGIC:
            return 0, disk_size
        if s0[510:512] != b'\x55\xAA':
            raise RockFSError('no RockFS volume found (no partition table and no RockFS at the start)')
        for i in range(4):
            e = s0[446 + i * 16:462 + i * 16]
            ptype = e[4]
            start, count = struct.unpack('<II', e[8:16])
            if ptype == MBR_TYPE and count:
                return start * SECTOR, count * SECTOR
            if ptype == 0xEE:
                return self._find_gpt()
        raise RockFSError('no RockFS partition found')

    def _find_gpt(self):
        hdr = self._read_at(SECTOR, SECTOR)
        if hdr[:8] != b'EFI PART':
            raise RockFSError('damaged GPT header')
        table_lba, n, esize = struct.unpack('<QII', hdr[72:88])
        for i in range(n):
            e = self._read_at(table_lba * SECTOR + i * esize, esize)
            if e[:16] == GPT_TYPE:
                first, last = struct.unpack('<QQ', e[32:48])
                return first * SECTOR, (last - first + 1) * SECTOR
        raise RockFSError('no RockFS partition found')

    def read_block(self, n):
        if n >= self.length // BLOCK:
            raise RockFSError('block %d is outside the volume' % n)
        return self._read_at(self.offset + n * BLOCK, BLOCK)

    def write_block(self, n, data, offset=0):
        if not self.writable:
            raise RockFSError('the volume is read-only')
        if n >= self.length // BLOCK:
            raise RockFSError('block %d is outside the volume' % n)
        self.f.seek(self.offset + n * BLOCK + offset)
        self.f.write(data)

    def sync(self):
        self.f.flush()
        os.fsync(self.f.fileno())

    # ---------------------------------------------------------------- superblock
    def _parse_sb(self, raw):
        f = list(struct.unpack(SB_FMT, raw))
        if f[0] != MAGIC:
            return None
        if f[-1] != crc(raw[:1020]):
            return None
        keys = ['magic', 'version', 'block_shift', 'name_rule', 'pad', 'block_count', 'bitmap_start',
                'bitmap_blocks', 'data_start', 'free_blocks', 'reserve_blocks', 'inode_count',
                'free_inodes', 'root_inode', 'state', 'features_compat', 'features_ro_compat',
                'features_incompat', 'mount_count', 'created', 'mounted', 'uuid', 'label',
                'inode_table', 'checksum']
        sb = dict(zip(keys, f))
        sb['table'] = Inode.unpack(sb['inode_table'], verify=False)
        return sb

    def _load_superblock(self):
        sb = self._parse_sb(self._read_at(self.offset, 1024))
        self.used_backup = False
        if sb is None:
            last = self.length // BLOCK - 1
            sb = self._parse_sb(self._read_at(self.offset + last * BLOCK, 1024))
            if sb is None:
                raise RockFSError('the RockFS superblock and its backup are both damaged')
            self.used_backup = True
        if sb['version'] != VERSION or sb['block_shift'] != 12:
            raise RockFSError('unsupported RockFS version %d' % sb['version'])
        if sb['features_incompat']:
            raise RockFSError('this volume uses features this tool does not understand')
        if (sb['features_ro_compat'] or sb['name_rule'] != 1) and self.writable:
            self.writable = False
        self.sb = sb
        self._load_bitmap()

    def _pack_sb(self):
        s = self.sb
        raw = struct.pack(SB_FMT, MAGIC, VERSION, 12, s['name_rule'], 0, s['block_count'], s['bitmap_start'],
                          s['bitmap_blocks'], s['data_start'], s['free_blocks'], s['reserve_blocks'],
                          s['inode_count'], s['free_inodes'], s['root_inode'], s['state'], s['features_compat'],
                          s['features_ro_compat'], s['features_incompat'], s['mount_count'], s['created'],
                          s['mounted'], s['uuid'], s['label'], s['table'].pack(), 0)
        return raw[:1020] + struct.pack('<I', crc(raw[:1020]))

    def write_superblock(self, backup=False):
        raw = self._pack_sb()
        if backup:  # the backup goes first, so one copy is always complete
            self.write_block(self.sb['block_count'] - 1, raw)
            self.sync()
        self.write_block(0, raw)
        self.sync()

    # ---------------------------------------------------------------- bitmap
    def _load_bitmap(self):
        s = self.sb
        raw = b''.join(self.read_block(s['bitmap_start'] + i) for i in range(s['bitmap_blocks']))
        self.bitmap = bytearray(raw)
        self.dirty_bitmap = set()
        free = 0
        for n in range(s['block_count']):
            if not self.bitmap[n >> 3] & (1 << (n & 7)):
                free += 1
        s['free_blocks'] = free

    def used(self, n):
        return bool(self.bitmap[n >> 3] & (1 << (n & 7)))

    def _set(self, n, value):
        if value:
            self.bitmap[n >> 3] |= 1 << (n & 7)
        else:
            self.bitmap[n >> 3] &= ~(1 << (n & 7)) & 0xFF
        self.dirty_bitmap.add(n // (BLOCK * 8))

    def _flush_bitmap(self):
        for b in sorted(self.dirty_bitmap):
            self.write_block(self.sb['bitmap_start'] + b, bytes(self.bitmap[b * BLOCK:(b + 1) * BLOCK]))
        self.dirty_bitmap.clear()
        self.sync()

    def mark(self, runs, value):
        for start, length in runs:
            for n in range(start, start + length):
                if self.used(n) == value:
                    raise RockFSError('bitmap is inconsistent at block %d' % n)
                self._set(n, value)
            self.sb['free_blocks'] += -length if value else length
        self._flush_bitmap()

    def allocate(self, count, privileged=False):
        """Find `count` free blocks, preferring one run. Marks them used."""
        s = self.sb
        avail = s['free_blocks'] - (0 if privileged else s['reserve_blocks'])
        if count > avail:
            raise RockFSError('Not enough space')
        runs, need, n = [], count, s['data_start']
        while need and n < s['block_count'] - 1:
            if not self.used(n):
                start = n
                while n < s['block_count'] - 1 and not self.used(n) and n - start < need:
                    n += 1
                runs.append((start, n - start))
                need -= n - start
            else:
                n += 1
        if need:
            raise RockFSError('Not enough space')
        self.mark(runs, True)
        return runs

    # ---------------------------------------------------------------- inodes
    def table_extents(self):
        exts, _ = self.load_extents(self.sb['table'])
        return exts

    def map_block(self, ino, k):
        entries, depth = ino.entries, ino.depth
        while True:
            pick = None
            for e in entries:
                if e[0] <= k:
                    pick = e
                else:
                    break
            if pick is None:
                return 0
            if depth == 0:
                fb, db, ln = pick
                return db + (k - fb) if k < fb + ln else 0
            entries, depth = self._read_xblock(pick[1])

    def _read_xblock(self, n):
        raw = self.read_block(n)
        magic, count, depth, _, check = struct.unpack(XB_HDR, raw[:16])
        if magic != b'RXTB' or count > PER_XBLOCK:
            raise RockFSError('damaged extent block %d' % n)
        if check != crc(raw[:12] + b'\0\0\0\0' + raw[16:]):
            raise RockFSError('extent block %d checksum mismatch' % n)
        out = []
        for i in range(count):
            e = raw[16 + i * 24:40 + i * 24]
            if depth == 0:
                fb, db, ln, _ = struct.unpack(EXT_FMT, e)
                out.append((fb, db, ln))
            else:
                out.append(struct.unpack(IDX_FMT, e))
        return out, depth

    def load_extents(self, ino):
        """Returns (flat list of extents, list of extent-tree blocks)."""
        exts, tree = [], []

        def walk(entries, depth):
            if depth == 0:
                exts.extend(entries)
                return
            for e in entries:
                tree.append(e[1])
                child, cdepth = self._read_xblock(e[1])
                if cdepth != depth - 1:
                    raise RockFSError('extent tree has the wrong depth')
                walk(child, cdepth)
        walk(ino.entries, ino.depth)
        return exts, tree

    def inode_location(self, n):
        fb = n // INODES_PER_BLOCK
        db = self.map_block(self.sb['table'], fb)
        if not db:
            raise RockFSError('inode %d is outside the inode table' % n)
        return db, (n % INODES_PER_BLOCK) * 256

    def read_inode(self, n, verify=True):
        if n == TABLE_INO:
            return self.sb['table']
        if n <= 0 or n >= self.sb['inode_count']:
            raise RockFSError('invalid inode number %d' % n)
        db, off = self.inode_location(n)
        return Inode.unpack(self.read_block(db)[off:off + 256], verify)

    def write_inode(self, n, ino):
        """The commit point for any change to a file or folder."""
        if n == TABLE_INO:
            self.sb['table'] = ino
            self.write_superblock(backup=True)
            return
        db, off = self.inode_location(n)
        self.write_block(db, ino.pack(), off)
        self.sync()

    def alloc_inode(self, itype, parent):
        s = self.sb
        for n in range(2, s['inode_count']):
            ino = self.read_inode(n, verify=False)
            if ino.type == T_FREE:
                break
        else:
            # grow the inode table by one block of 16 inodes
            table = s['table']
            self.write_range(TABLE_INO, table, table.size, bytes(BLOCK), table.size + BLOCK)
            n = s['inode_count']
            s['inode_count'] += INODES_PER_BLOCK
            s['free_inodes'] += INODES_PER_BLOCK
            self.write_superblock(backup=True)
            ino = self.read_inode(n, verify=False)
        gen = ino.generation
        ino = Inode()
        ino.type, ino.parent, ino.generation = itype, parent, gen
        ino.created = ino.modified = now()
        self.write_inode(n, ino)
        s['free_inodes'] -= 1
        return n, ino

    def free_inode(self, n, ino):
        exts, tree = self.load_extents(ino)
        gen = ino.generation + 1
        dead = Inode()
        dead.generation = gen & 0xFFFFFFFF
        self.write_inode(n, dead)
        self.mark([(db, ln) for _, db, ln in exts] + [(t, 1) for t in tree], False)
        self.sb['free_inodes'] += 1

    # ---------------------------------------------------------------- writing contents (copy-on-write)
    def write_range(self, n, ino, off, data, new_size, privileged=False):
        """Writes `data` at `off` and sets the size to `new_size`, never overwriting
        any block the file currently uses. Commits by writing the inode."""
        old_exts, old_tree = self.load_extents(ino)
        end_block = -(-new_size // BLOCK)
        if data:
            b0, b1 = off // BLOCK, (off + len(data) - 1) // BLOCK
        else:
            b0, b1 = end_block, end_block - 1
        b1 = min(b1, end_block - 1)

        # what survives: everything outside [b0, b1] and below the new end
        keep, freed = [], []
        for fb, db, ln in old_exts:
            for start, length in ((fb, ln),):
                s0, s1 = start, start + length  # [s0, s1)
                pieces = [(s0, min(s1, b0)), (max(s0, b1 + 1), s1)] if b1 >= b0 else [(s0, s1)]
                kept_any = set()
                for p0, p1 in pieces:
                    p1 = min(p1, end_block)
                    if p1 > p0:
                        keep.append((p0, db + (p0 - fb), p1 - p0))
                        kept_any.update(range(p0, p1))
                for k in range(s0, s1):
                    if k not in kept_any:
                        freed.append((db + (k - fb), 1))

        count = b1 - b0 + 1 if b1 >= b0 else 0
        runs = []
        if count:
            # room check before anything is changed
            est = tree_blocks_needed(len(keep) + 1)  # exact need is checked when the tree is built
            if count + est > self.sb['free_blocks'] - (0 if privileged else self.sb['reserve_blocks']):
                raise RockFSError('Not enough space to save safely')
            runs = self.allocate(count, privileged)
            k = b0
            for start, length in runs:
                for i in range(length):
                    blk = bytearray(BLOCK)
                    lo, hi = k * BLOCK, (k + 1) * BLOCK
                    if lo < off or hi > off + len(data):  # partial: start from the old contents
                        old = self.map_block(ino, k)
                        if old:
                            blk[:] = self.read_block(old)
                    s = max(lo, off)
                    e = min(hi, off + len(data))
                    blk[s - lo:e - lo] = data[s - off:e - off]
                    if hi > new_size:
                        cut = max(0, new_size - lo)
                        blk[cut:] = bytes(BLOCK - cut)
                    self.write_block(start + i, bytes(blk))
                    keep.append((k, start + i, 1))
                    k += 1
            self.sync()
        new_exts = merge_extents(keep)

        try:
            entries, depth, new_tree = self._build_tree(new_exts, privileged)
        except RockFSError:
            self.mark(runs, False)
            raise
        ino.entries, ino.depth = entries, depth
        ino.size = new_size
        ino.modified = now()
        self.write_inode(n, ino)
        # finally, release what the file no longer uses
        self.mark(merge_runs(freed) + [(t, 1) for t in old_tree], False)

    def _build_tree(self, exts, privileged):
        if len(exts) <= IN_INODE:
            return exts, 0, []
        need = tree_blocks_needed(len(exts))
        runs = self.allocate(need, privileged)
        blocks = [s + i for s, ln in runs for i in range(ln)]
        level, depth, used = exts, 0, []
        while len(level) > IN_INODE:
            upper = []
            for i in range(0, len(level), PER_XBLOCK):
                chunk = level[i:i + PER_XBLOCK]
                blk = blocks.pop(0)
                used.append(blk)
                body = b''
                for e in chunk:
                    body += struct.pack(EXT_FMT, e[0], e[1], e[2], 0) if depth == 0 else struct.pack(IDX_FMT, e[0], e[1])
                body = body.ljust(BLOCK - 16, b'\0')
                hdr = struct.pack(XB_HDR, b'RXTB', len(chunk), depth, 0, 0)
                check = crc(hdr + body)
                self.write_block(blk, struct.pack(XB_HDR, b'RXTB', len(chunk), depth, 0, check) + body)
                upper.append((chunk[0][0], blk))
            level, depth = upper, depth + 1
        self.sync()
        return level, depth, used

    def read_file(self, ino):
        out = bytearray()
        for k in range(-(-ino.size // BLOCK)):
            db = self.map_block(ino, k)
            out += self.read_block(db) if db else bytes(BLOCK)
        return bytes(out[:ino.size])

    # ---------------------------------------------------------------- folders
    def dir_entries(self, n, ino=None):
        """Yields (file_block, offset, inode, type, name) for every name in a folder."""
        ino = ino or self.read_inode(n)
        if ino.type != T_DIR:
            raise RockFSError('not a folder')
        for k in range(ino.size // BLOCK):
            db = self.map_block(ino, k)
            raw = self.read_block(db) if db else bytes(BLOCK)
            pos = 0
            while pos < BLOCK:
                inum, elen, nlen, etype = struct.unpack(DE_HDR, raw[pos:pos + 16])
                if elen < 16 or elen % 8 or pos + elen > BLOCK:
                    if elen == 0 and pos == 0 and not raw.strip(b'\0'):
                        break  # a never-written block
                    raise RockFSError('damaged folder entry in inode %d' % n)
                if inum:
                    yield k, pos, inum, etype, raw[pos + 16:pos + 16 + nlen].decode('utf-8', 'replace')
                pos += elen

    def lookup_in(self, d, name):
        key = name_key(name)
        for _, _, inum, etype, nm in self.dir_entries(d):
            if name_key(nm) == key:
                return inum
        return 0

    def dir_add(self, d, name, inum, itype):
        dino = self.read_inode(d)
        raw_name = name.encode('utf-8')
        need = align8(16 + len(raw_name))
        for k in range(dino.size // BLOCK):
            db = self.map_block(dino, k)
            blk = bytearray(self.read_block(db) if db else bytes(BLOCK))
            pos = 0
            while pos < BLOCK:
                einum, elen, nlen, etype = struct.unpack(DE_HDR, blk[pos:pos + 16])
                if elen == 0:
                    elen = BLOCK - pos  # a never-written block counts as one empty entry
                usedlen = align8(16 + nlen) if einum else 0
                if elen - usedlen >= need:
                    if einum:
                        struct.pack_into(DE_HDR, blk, pos, einum, usedlen, nlen, etype)
                        pos += usedlen
                        elen -= usedlen
                    blk[pos:pos + elen] = bytes(elen)
                    struct.pack_into(DE_HDR, blk, pos, inum, elen, len(raw_name), itype)
                    blk[pos + 16:pos + 16 + len(raw_name)] = raw_name
                    self.write_range(d, dino, k * BLOCK, bytes(blk), dino.size)
                    return
                pos += elen
        blk = bytearray(BLOCK)
        struct.pack_into(DE_HDR, blk, 0, inum, BLOCK, len(raw_name), itype)
        blk[16:16 + len(raw_name)] = raw_name
        self.write_range(d, dino, dino.size, bytes(blk), dino.size + BLOCK)

    def dir_remove(self, d, name, privileged=True):
        dino = self.read_inode(d)
        key = name_key(name)
        for k in range(dino.size // BLOCK):
            db = self.map_block(dino, k)
            blk = bytearray(self.read_block(db) if db else bytes(BLOCK))
            pos, prev = 0, None
            while pos < BLOCK:
                einum, elen, nlen, etype = struct.unpack(DE_HDR, blk[pos:pos + 16])
                if elen == 0:
                    break
                if einum and name_key(blk[pos + 16:pos + 16 + nlen].decode('utf-8', 'replace')) == key:
                    if prev is not None:  # merge into the entry before
                        pinum, plen, pnlen, ptype = struct.unpack(DE_HDR, blk[prev:prev + 16])
                        struct.pack_into(DE_HDR, blk, prev, pinum, plen + elen, pnlen, ptype)
                        blk[pos:pos + elen] = bytes(elen)
                    else:
                        blk[pos:pos + elen] = bytes(elen)
                        struct.pack_into(DE_HDR, blk, pos, 0, elen, 0, 0)
                    self.write_range(d, dino, k * BLOCK, bytes(blk), dino.size, privileged)
                    return True
                prev = pos
                pos += elen
        return False

    # ---------------------------------------------------------------- paths
    def resolve(self, path):
        n = self.sb['root_inode']
        for part in [p for p in path.split('/') if p and p != '.']:
            if part == '..':
                n = self.read_inode(n).parent
                continue
            ino = self.read_inode(n)
            if ino.type != T_DIR:
                raise RockFSError('%s: not a folder' % part)
            n = self.lookup_in(n, part)
            if not n:
                raise RockFSError('%s: not found' % path)
        return n

    def split(self, path):
        parts = [p for p in path.split('/') if p]
        if not parts:
            raise RockFSError('that is the top-level folder')
        return self.resolve('/'.join(parts[:-1])), check_name(parts[-1])

    def create(self, path, itype, data=b''):
        parent, name = self.split(path)
        if self.read_inode(parent).type != T_DIR:
            raise RockFSError('not a folder')
        if self.lookup_in(parent, name):
            raise RockFSError('%s already exists' % path)
        n, ino = self.alloc_inode(itype, parent)
        try:
            if data:
                self.write_range(n, ino, 0, data, len(data))
            self.dir_add(parent, name, n, itype)
        except RockFSError:
            self.free_inode(n, self.read_inode(n))
            raise
        return n

    def put(self, path, data):
        try:
            n = self.resolve(path)
        except RockFSError:
            return self.create(path, T_FILE, data)
        ino = self.read_inode(n)
        if ino.type != T_FILE:
            raise RockFSError('%s is a folder' % path)
        self.write_range(n, ino, 0, data, len(data))
        return n

    def remove(self, path):
        parent, name = self.split(path)
        n = self.lookup_in(parent, name)
        if not n:
            raise RockFSError('%s: not found' % path)
        ino = self.read_inode(n)
        if ino.type == T_DIR and any(True for _ in self.dir_entries(n, ino)):
            raise RockFSError('the folder is not empty')
        self.dir_remove(parent, name)
        self.free_inode(n, ino)

    def rename(self, src, dst):
        sp, sname = self.split(src)
        n = self.lookup_in(sp, sname)
        if not n:
            raise RockFSError('%s: not found' % src)
        dp, dname = self.split(dst)
        if self.lookup_in(dp, dname):
            raise RockFSError('%s already exists' % dst)
        ino = self.read_inode(n)
        p = dp
        while ino.type == T_DIR:  # a folder can't move inside itself
            if p == n:
                raise RockFSError('a folder can\'t be moved inside itself')
            if p == self.sb['root_inode']:
                break
            p = self.read_inode(p).parent
        self.dir_add(dp, dname, n, ino.type)
        ino = self.read_inode(n)
        ino.parent = dp
        self.write_inode(n, ino)
        self.dir_remove(sp, sname)

    # ---------------------------------------------------------------- checking
    def _scan(self):
        """Works out which blocks are really in use. Returns (used bitmap, live inodes, problems)."""
        s = self.sb
        problems = []
        used = bytearray(len(self.bitmap))
        owner = {}

        def claim(blk, who):
            if blk < s['data_start'] or blk >= s['block_count'] - 1:
                problems.append('%s uses block %d, which is outside the data area' % (who, blk))
                return
            if used[blk >> 3] & (1 << (blk & 7)):
                problems.append('%s and %s both use block %d' % (owner[blk], who, blk))
                return
            used[blk >> 3] |= 1 << (blk & 7)
            owner[blk] = who

        for b in [0, s['block_count'] - 1] + [s['bitmap_start'] + i for i in range(s['bitmap_blocks'])]:
            used[b >> 3] |= 1 << (b & 7)
            owner[b] = 'metadata'
        for n in range(s['block_count'], len(used) * 8):
            used[n >> 3] |= 1 << (n & 7)

        def claim_inode(who, ino):
            exts, tree = self.load_extents(ino)
            last = -1
            for fb, db, ln in exts:
                if fb <= last:
                    problems.append('%s has overlapping or unsorted extents' % who)
                last = fb + ln - 1
                for i in range(ln):
                    claim(db + i, who)
            for t in tree:
                claim(t, who + ' (extent tree)')

        claim_inode('the inode table', s['table'])
        live, bad = {}, []
        for n in range(1, s['inode_count']):
            try:
                ino = self.read_inode(n)
            except RockFSError as e:
                problems.append('inode %d: %s' % (n, e))
                bad.append(n)
                continue
            if ino.type == T_FREE:
                continue
            if ino.type not in (T_FILE, T_DIR):
                problems.append('inode %d has an unknown type %d' % (n, ino.type))
                bad.append(n)
                continue
            try:
                claim_inode('inode %d' % n, ino)
            except RockFSError as e:
                problems.append('inode %d: %s' % (n, e))
                bad.append(n)
                continue
            live[n] = ino
        return used, live, bad, problems

    def check(self, repair=False):
        """Returns a list of problems found (and fixed, if repair is set)."""
        s = self.sb
        problems = ['the main superblock was damaged; the backup was used'] if self.used_backup else []
        used, live, bad, found = self._scan()
        problems += found
        if repair:
            for n in bad:
                self.write_inode(n, Inode())
        root = s['root_inode']
        if root not in live or live[root].type != T_DIR:
            problems.append('the top-level folder is missing or damaged')
            return problems

        seen, queue, fixes = {root}, [root], []
        while queue:
            d = queue.pop(0)
            names = {}
            try:
                entries = list(self.dir_entries(d, live[d]))
            except RockFSError as e:
                problems.append(str(e))
                continue
            for _, _, inum, etype, name in entries:
                where = 'folder %d, "%s"' % (d, name)
                if inum not in live:
                    problems.append('%s points at a missing or damaged item (inode %d)' % (where, inum))
                    fixes.append(('unlink', d, name))
                    continue
                if inum in seen:
                    problems.append('%s is a second name for inode %d' % (where, inum))
                    fixes.append(('unlink', d, name))
                    continue
                key = name_key(name)
                if key in names:
                    problems.append('folder %d has two names that count as "%s"' % (d, name))
                names[key] = inum
                seen.add(inum)
                ino = live[inum]
                if etype != ino.type:
                    problems.append('%s is recorded with the wrong type' % where)
                if ino.parent != d:
                    problems.append('inode %d says its folder is %d, but it is in %d' % (inum, ino.parent, d))
                    fixes.append(('parent', inum, d))
                if ino.type == T_DIR:
                    queue.append(inum)
        orphans = sorted(n for n in live if n not in seen)
        if orphans:
            problems.append('%d item(s) belong to no folder' % len(orphans))
        runs = {True: [], False: []}
        for n in range(s['block_count']):
            a = bool(used[n >> 3] & (1 << (n & 7)))
            if a != self.used(n):
                r = runs[a]
                if r and r[-1][1] == n - 1:
                    r[-1][1] = n
                else:
                    r.append([n, n])
        for a, text in ((True, 'in use but marked free'), (False, 'marked used but belong to nothing')):
            for lo, hi in runs[a]:
                problems.append(('block %d is %s' % (lo, text.replace('belong ', 'belongs '))) if lo == hi else
                                ('blocks %d-%d are %s' % (lo, hi, text)))
        counted = sum(1 for n in range(2, s['inode_count']) if self.read_inode(n, verify=False).type == T_FREE)

        if repair and problems:
            for fix in fixes:
                if fix[0] == 'unlink':
                    self.dir_remove(fix[1], fix[2])
                else:
                    ino = self.read_inode(fix[1])
                    ino.parent = fix[2]
                    self.write_inode(fix[1], ino)
            # the bitmap must be right before anything new is allocated
            self._write_bitmap(self._scan()[0])
            if orphans:
                keep = []
                for n in orphans:
                    ino = self.read_inode(n)
                    if ino.size == 0:  # an empty leftover: just remove it
                        self.free_inode(n, ino)
                    else:
                        keep.append(n)
                lf = None
                if keep:
                    try:
                        lf = self.resolve('/lost+found')
                    except RockFSError:
                        lf = self.create('/lost+found', T_DIR)
                for n in keep:
                    ino = self.read_inode(n)
                    self.dir_add(lf, '#%d' % n, n, ino.type)
                    ino = self.read_inode(n)
                    ino.parent = lf
                    self.write_inode(n, ino)
                self._write_bitmap(self._scan()[0])
            counted = sum(1 for n in range(2, s['inode_count']) if self.read_inode(n, verify=False).type == T_FREE)
        if repair:
            s['free_inodes'] = counted
            s['state'] = 0
            self.write_superblock(backup=True)
        return problems

    def _write_bitmap(self, used):
        self.bitmap = bytearray(used)
        self.dirty_bitmap = set(range(self.sb['bitmap_blocks']))
        self._flush_bitmap()
        self._load_bitmap()

    def close(self, clean=True):
        if self.writable and clean:
            self.write_superblock(backup=True)
        self.f.close()


def merge_runs(runs):
    runs = sorted(runs)
    out = []
    for s, ln in runs:
        if out and out[-1][0] + out[-1][1] == s:
            out[-1] = (out[-1][0], out[-1][1] + ln)
        else:
            out.append((s, ln))
    return out


def mkfs(path, size=32 * 1024 * 1024, label='Data', whole_disk=False):
    if not os.path.exists(path) or os.path.getsize(path) < size:
        with open(path, 'ab') as f:
            f.truncate(size)
    disk_size = os.path.getsize(path)
    with open(path, 'r+b') as f:
        if whole_disk:
            start, sectors = 0, disk_size // SECTOR
        else:
            start, sectors = PART_START, disk_size // SECTOR - PART_START
            mbr = bytearray(SECTOR)
            entry = struct.pack('<B3sB3sII', 0, b'\xff\xff\xff', MBR_TYPE, b'\xff\xff\xff', start, sectors)
            mbr[446:462] = entry
            mbr[440:444] = os.urandom(4)
            mbr[510:512] = b'\x55\xAA'
            f.seek(0)
            f.write(mbr)
        format_volume(f, start * SECTOR, sectors * SECTOR, label)


def format_volume(f, offset, length, label):
    block_count = length // BLOCK
    if block_count < 64:
        raise RockFSError('the volume is too small for RockFS (needs at least 256 KiB)')
    bitmap_blocks = -(-block_count // (BLOCK * 8))
    data_start = 1 + bitmap_blocks
    table_block = data_start
    bitmap = bytearray(bitmap_blocks * BLOCK)

    def setbit(n):
        bitmap[n >> 3] |= 1 << (n & 7)
    for n in list(range(0, data_start)) + [block_count - 1, table_block]:
        setbit(n)
    for n in range(block_count, bitmap_blocks * BLOCK * 8):
        setbit(n)
    t = now()
    root = Inode()
    root.type, root.parent, root.created, root.modified = T_DIR, 1, t, t
    table_block_data = bytearray(BLOCK)
    table_block_data[256:512] = root.pack()
    table = Inode()
    table.type, table.parent, table.size = T_FILE, 0, BLOCK
    table.created = table.modified = t
    table.entries = [(0, table_block, 1)]

    def wb(n, data):
        f.seek(offset + n * BLOCK)
        f.write(data)
    wb(table_block, bytes(table_block_data))
    for i in range(bitmap_blocks):
        wb(1 + i, bytes(bitmap[i * BLOCK:(i + 1) * BLOCK]))
    used = data_start + 2
    sb = struct.pack(SB_FMT, MAGIC, VERSION, 12, 1, 0, block_count, 1, bitmap_blocks, data_start,
                     block_count - used, max(16, block_count // 100), INODES_PER_BLOCK, INODES_PER_BLOCK - 2,
                     1, 0, 0, 0, 0, 0, t, 0, uuid.uuid4().bytes, label.encode('utf-8')[:63].ljust(64, b'\0'),
                     table.pack(), 0)
    sb = sb[:1020] + struct.pack('<I', crc(sb[:1020]))
    wb(block_count - 1, sb.ljust(BLOCK, b'\0'))
    wb(0, sb.ljust(BLOCK, b'\0'))
    f.flush()


def human(n):
    for unit in ('bytes', 'KB', 'MB', 'GB', 'TB'):
        if n < 1024 or unit == 'TB':
            return ('%d %s' % (n, unit)) if unit == 'bytes' else ('%.1f %s' % (n, unit))
        n /= 1024.0


def main(argv):
    if len(argv) < 3 or argv[1] in ('-h', '--help'):
        print(__doc__.strip())
        return 0 if len(argv) > 1 else 1
    image, cmd, args = argv[1], argv[2], argv[3:]
    try:
        if cmd == 'mkfs':
            size, label, whole = 32 * 1024 * 1024, 'Data', False
            i = 0
            while i < len(args):
                if args[i] == '--size':
                    v = args[i + 1].upper()
                    mult = {'K': 1 << 10, 'M': 1 << 20, 'G': 1 << 30}.get(v[-1], 1)
                    size = int(v.rstrip('KMG')) * mult
                    i += 2
                elif args[i] == '--label':
                    label = args[i + 1]
                    i += 2
                elif args[i] == '--whole-disk':
                    whole = True
                    i += 1
                else:
                    raise RockFSError('unknown option ' + args[i])
            mkfs(image, size, label, whole)
            print('Formatted %s as RockFS (%s)' % (image, human(size)))
            return 0
        write_cmds = ('put', 'mkdir', 'rm', 'mv')
        repair = cmd == 'check' and '--repair' in args
        vol = Volume(image, writable=cmd in write_cmds or repair)
        if cmd in write_cmds + ('check',) and repair is False and cmd != 'check' and not vol.writable:
            raise RockFSError('this volume can only be read by this version of the tool')
        s = vol.sb
        if cmd == 'info':
            files = sum(1 for n in range(1, s['inode_count']) if vol.read_inode(n, False).type != T_FREE)
            print('Label:      %s' % s['label'].rstrip(b'\0').decode('utf-8', 'replace'))
            print('ID:         %s' % uuid.UUID(bytes=s['uuid']))
            print('Size:       %s (%d blocks of 4 KiB)' % (human(s['block_count'] * BLOCK), s['block_count']))
            print('Free:       %s (%s kept in reserve)' % (human(s['free_blocks'] * BLOCK), human(s['reserve_blocks'] * BLOCK)))
            print('Items:      %d files and folders (room for %d more before the table grows)' % (files, s['free_inodes']))
            print('Created:    %s' % time.strftime('%Y-%m-%d %H:%M UTC', time.gmtime(s['created'])))
            print('Mounted:    %d times, last %s' % (s['mount_count'], time.strftime('%Y-%m-%d %H:%M UTC', time.gmtime(s['mounted'])) if s['mounted'] else 'never'))
            print('Shut down:  %s' % ('cleanly' if s['state'] == 0 else 'NOT cleanly; run "check"'))
        elif cmd == 'ls':
            rec = '-R' in args
            paths = [a for a in args if a != '-R'] or ['/']

            def show(path, n, indent):
                rows = sorted(vol.dir_entries(n), key=lambda e: (e[3] != T_DIR, name_key(e[4])))
                for _, _, inum, etype, name in rows:
                    ino = vol.read_inode(inum)
                    when = time.strftime('%Y-%m-%d %H:%M', time.gmtime(ino.modified))
                    if etype == T_DIR:
                        print('%s%-40s %12s  %s' % (indent, name + '/', '', when))
                        if rec:
                            show(path.rstrip('/') + '/' + name, inum, indent + '  ')
                    else:
                        print('%s%-40s %12s  %s' % (indent, name, human(ino.size), when))
            for p in paths:
                show(p, vol.resolve(p), '')
        elif cmd == 'cat':
            sys.stdout.buffer.write(vol.read_file(vol.read_inode(vol.resolve(args[0]))))
        elif cmd == 'get':
            data = vol.read_file(vol.read_inode(vol.resolve(args[0])))
            dest = args[1] if len(args) > 1 else os.path.basename(args[0])
            with open(dest, 'wb') as f:
                f.write(data)
            print('Copied %s to %s (%s)' % (args[0], dest, human(len(data))))
        elif cmd == 'put':
            with open(args[0], 'rb') as f:
                data = f.read()
            vol.put(args[1], data)
            print('Copied %s to %s (%s)' % (args[0], args[1], human(len(data))))
        elif cmd == 'mkdir':
            vol.create(args[0], T_DIR)
        elif cmd == 'rm':
            vol.remove(args[0])
        elif cmd == 'mv':
            vol.rename(args[0], args[1])
        elif cmd == 'check':
            problems = vol.check(repair)
            if not problems:
                print('No problems found.')
            for p in problems:
                print(('Fixed: ' if repair else 'Problem: ') + p)
            vol.close(clean=False)
            return 0 if not problems or repair else 2
        else:
            raise RockFSError('unknown command "%s"' % cmd)
        vol.close(clean=cmd in write_cmds)
        return 0
    except RockFSError as e:
        print('rockfs: %s' % e, file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main(sys.argv))
