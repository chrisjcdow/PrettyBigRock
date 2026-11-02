// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! A mounted RockFS volume: mounting and unmounting it, and the plumbing every other module
//! uses to reach the disk.
//!
//! To **mount** a volume is to open it for use: find and check its superblock, load the bitmap
//! and the inode table's location into memory, and mark the volume "in use" on the disk. To
//! **unmount** is the reverse, and the last thing it does is mark the volume "shut down
//! cleanly". If the machine stops in between, the volume still says "in use" at the next mount,
//! and the mount checks and repairs it first (ROCKFS.md, "Saving safely"; note 0016, section 2).

use crate::bitmap::Bitmap;
use crate::content::Pending;
use crate::error::{Info, MountError, Problem};
use crate::extents::lookup;
use crate::inode::Inode;
use crate::layout::*;
use crate::runs::Run;
use crate::superblock::{MAGIC, Superblock};
use crate::tree::Loaded;
use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;
use pbr_fs::{BlockDevice, FsError, NodeId, SECTOR};

/// A mounted RockFS volume on a block device `D`.
///
/// `D` is a *type parameter*: `RockFs` works with any type that implements the [`BlockDevice`]
/// trait (Rust's word for an interface), such as a real disk in a filesystem server or a disk
/// image in memory in a test. Rust makes a separate, fully optimised copy of this code for
/// each `D` it is used with, so the flexibility costs nothing at run time.
pub struct RockFs<D: BlockDevice> {
    /// The disk.
    pub(crate) dev: D,
    /// Seconds since 1970 UTC.
    pub(crate) clock: fn() -> u64,
    /// The superblock as it is (or, between writes, will be) on the disk.
    pub(crate) sb: Superblock,
    /// Whether every change is refused.
    pub(crate) read_only: bool,
    /// Set when a write to the disk failed. The in-memory picture and the disk may then
    /// disagree, so every later change is refused, and the volume is not marked clean.
    pub(crate) poisoned: bool,
    /// Which blocks are in use. Written back lazily.
    pub(crate) bitmap: Bitmap,
    /// Where the inode table is.
    pub(crate) table: Loaded,
    /// Files with changes not yet committed, by inode number (note 0014, section 6).
    pub(crate) pending: BTreeMap<u32, Pending>,
    /// The extent list of the file or folder used last, so reading a big file in pieces
    /// doesn't read its tree again for every piece: (inode, generation, what was loaded).
    pub(crate) cache: Option<(u32, u32, Loaded)>,
    /// Inodes below this number are known to be in use, so looking for a free one starts here.
    pub(crate) inode_hint: u32,
    /// The root folder's `NodeId`.
    pub(crate) root: NodeId,
    /// Whether the volume was not shut down cleanly when it was mounted.
    pub(crate) unclean_at_mount: bool,
    /// Whether the main superblock was damaged and the backup was used at mount.
    pub(crate) used_backup: bool,
    /// Whether the main superblock still needs rewriting because of that (a repair does it).
    pub(crate) main_damaged: bool,
    /// The `mounted` time the superblock held before this mount.
    pub(crate) last_mounted: u64,
    /// What the repair at mount found, if it ran.
    pub(crate) mount_report: Vec<Problem>,
}

impl<D: BlockDevice> RockFs<D> {
    /// Mounts the volume on `dev` (sector 0 of `dev` is the volume's first sector).
    ///
    /// `clock` gives the time in seconds since 1970 UTC; it is called whenever something is
    /// created or changed. The volume is mounted read-only if `read_only` is set, if the device
    /// is read-only, or if the volume uses a feature this code knows it must not write (an
    /// unknown `features_ro_compat` bit or `name_rule`). A volume that was not shut down
    /// cleanly is checked and repaired before it is used, unless it is read-only, in which case
    /// nothing can be repaired and [`RockFs::check`] shows what is wrong.
    pub fn mount(dev: D, clock: fn() -> u64, read_only: bool) -> Result<RockFs<D>, MountError> {
        let device_blocks = dev.sectors() / SECTORS_PER_BLOCK;
        if device_blocks < 2 {
            return Err(MountError::NotRockFs);
        }
        let mut dev = dev;
        // The main superblock is block 0; the backup is in the last block of the device.
        let mut heads = [[0u8; SUPERBLOCK_SIZE]; 2];
        let places = [0, (device_blocks - 1) * SECTORS_PER_BLOCK];
        for (head, sector) in heads.iter_mut().zip(places) {
            dev.read(sector, head).map_err(|_| MountError::Disk)?;
        }
        let (sb, used_backup) = match (Superblock::parse(&heads[0]), Superblock::parse(&heads[1])) {
            (Some(sb), _) => (sb, false),
            (None, Some(sb)) => (sb, true),
            (None, None) if heads.iter().any(|h| h[..8] == MAGIC) => {
                return Err(MountError::Damaged("neither superblock is intact"));
            }
            (None, None) => return Err(MountError::NotRockFs),
        };
        if sb.version != 1 {
            return Err(MountError::Unsupported("a version of RockFS this code does not know"));
        }
        if sb.block_shift != 12 {
            return Err(MountError::Unsupported("a block size other than 4 KiB"));
        }
        if sb.features_incompat != 0 {
            return Err(MountError::Unsupported("features this code does not know"));
        }
        check_geometry(&sb, device_blocks)?;
        let ro = read_only || dev.read_only() || sb.features_ro_compat != 0 || sb.name_rule != 1;
        let bitmap = Bitmap::empty(sb.block_count);
        let mut fs = RockFs {
            dev,
            clock,
            last_mounted: sb.mounted,
            unclean_at_mount: sb.state != 0 || used_backup,
            sb,
            read_only: ro,
            poisoned: false,
            bitmap,
            table: Loaded::default(),
            pending: BTreeMap::new(),
            cache: None,
            inode_hint: 2,
            root: 0,
            used_backup,
            main_damaged: used_backup,
            mount_report: Vec::new(),
        };
        fs.load_bitmap().map_err(|e| mount_error(e, "the block bitmap is unreadable"))?;
        fs.load_table()?;
        let root = fs
            .read_inode(fs.sb.root_inode, true)
            .map_err(|e| mount_error(e, "the top-level folder's inode is unreadable"))?;
        if root.kind != TYPE_FOLDER {
            return Err(MountError::Damaged("the top-level folder is not a folder"));
        }
        fs.root = node_id(fs.sb.root_inode, root.generation);
        if !fs.read_only {
            if fs.unclean_at_mount {
                // The bitmap on the disk may be behind, and there may be space or items left
                // by an unfinished save. Repair before anything else is allowed to allocate.
                fs.mount_report = fs
                    .check_inner(true)
                    .map_err(|e| mount_error(e, "the volume could not be repaired"))?;
            }
            fs.sb.state = 1;
            fs.sb.mount_count = fs.sb.mount_count.wrapping_add(1);
            fs.sb.mounted = (fs.clock)();
            fs.sb.free_blocks = fs.bitmap.free();
            // Both copies say "in use" from now on, backup first as for any structural
            // change, so that whichever is used after a crash tells the truth.
            let sb = fs.sb.clone();
            fs.write_superblocks(&sb)
                .map_err(|e| mount_error(e, "the superblock is unwritable"))?;
        }
        Ok(fs)
    }

    /// What the check at mount found and fixed, if the volume needed one.
    pub fn mount_report(&self) -> &[Problem] {
        &self.mount_report
    }

    /// Facts about the volume.
    pub fn info(&self) -> Info {
        Info {
            label: self.sb.label_text(),
            uuid: self.sb.uuid,
            block_count: self.sb.block_count,
            free_blocks: self.bitmap.free(),
            reserve_blocks: self.sb.reserve_blocks,
            inode_count: self.sb.inode_count,
            free_inodes: self.sb.free_inodes,
            mount_count: self.sb.mount_count,
            created: self.sb.created,
            last_mounted: self.last_mounted,
            clean: !self.unclean_at_mount,
            used_backup: self.used_backup,
            read_only: self.read_only,
            name_rule: self.sb.name_rule,
        }
    }

    /// Gives the disk back, without writing anything. Unmount first
    /// ([`Filesystem::unmount`](pbr_fs::Filesystem::unmount)) to leave the volume clean; this
    /// alone leaves it as if the machine had stopped now, which is what the tests use to look at
    /// the volume after a crash.
    pub fn into_device(self) -> D {
        self.dev
    }

    /// A look at the disk, for tests.
    pub fn device(&self) -> &D {
        &self.dev
    }

    /// The disk, for tests that need to change how it behaves.
    pub fn device_mut(&mut self) -> &mut D {
        &mut self.dev
    }

    // ------------------------------------------------------------------ reaching the disk

    /// The time now.
    pub(crate) fn now(&self) -> u64 {
        (self.clock)()
    }

    /// Fails if changes are not allowed: `ReadOnly` for a read-only mount, `Disk` after a failed
    /// write.
    pub(crate) fn writable(&self) -> Result<(), FsError> {
        if self.read_only {
            Err(FsError::ReadOnly)
        } else if self.poisoned {
            Err(FsError::Disk)
        } else {
            Ok(())
        }
    }

    /// Reads whole sectors, in pieces of at most 64 KiB (the most a device accepts at once).
    pub(crate) fn dev_read(&mut self, sector: u64, buf: &mut [u8]) -> Result<(), FsError> {
        let mut at = sector;
        for piece in buf.chunks_mut(MAX_TRANSFER) {
            self.dev.read(at, piece)?;
            at += (piece.len() / SECTOR) as u64;
        }
        Ok(())
    }

    /// Writes whole sectors. A failure marks the volume poisoned (see [`RockFs::poisoned`]).
    pub(crate) fn dev_write(&mut self, sector: u64, data: &[u8]) -> Result<(), FsError> {
        let mut at = sector;
        for piece in data.chunks(MAX_TRANSFER) {
            if let Err(e) = self.dev.write(at, piece) {
                self.poisoned = true;
                return Err(e.into());
            }
            at += (piece.len() / SECTOR) as u64;
        }
        Ok(())
    }

    /// Waits until everything written so far is on the disk. The disk may reorder writes freely
    /// between flushes, so a flush goes wherever one write must be safely on the disk before
    /// another is made (note 0016, section 2).
    pub(crate) fn dev_flush(&mut self) -> Result<(), FsError> {
        if let Err(e) = self.dev.flush() {
            self.poisoned = true;
            return Err(e.into());
        }
        Ok(())
    }

    /// Reads `out.len() / BLOCK` blocks starting at block `start`.
    pub(crate) fn read_blocks(&mut self, start: u64, out: &mut [u8]) -> Result<(), FsError> {
        self.check_range(start, out.len())?;
        self.dev_read(start * SECTORS_PER_BLOCK, out)
    }

    /// Reads one block.
    pub(crate) fn read_block(
        &mut self,
        block: u64,
    ) -> Result<alloc::boxed::Box<[u8; BLOCK]>, FsError> {
        let mut buf = alloc::boxed::Box::new([0u8; BLOCK]);
        self.read_blocks(block, &mut buf[..])?;
        Ok(buf)
    }

    /// Writes `data.len() / BLOCK` blocks starting at block `start`.
    pub(crate) fn write_blocks(&mut self, start: u64, data: &[u8]) -> Result<(), FsError> {
        self.check_range(start, data.len())?;
        self.dev_write(start * SECTORS_PER_BLOCK, data)
    }

    /// Fails with `Damaged` unless `len` bytes from block `start` lie inside the volume.
    fn check_range(&self, start: u64, len: usize) -> Result<(), FsError> {
        let blocks = (len / BLOCK) as u64;
        match start.checked_add(blocks) {
            Some(end) if end <= self.sb.block_count => Ok(()),
            _ => Err(FsError::Damaged),
        }
    }

    /// Writes the superblock to both places: the backup first, a flush, then the main copy, a
    /// flush. A change to the volume's structure is written this way so that if the machine
    /// stops part-way, one of the two copies is always complete and has a good checksum
    /// (ROCKFS.md, "The superblock"). On success `sb` becomes the volume's superblock.
    pub(crate) fn write_superblocks(&mut self, sb: &Superblock) -> Result<(), FsError> {
        let bytes = sb.to_bytes();
        self.dev_write((sb.block_count - 1) * SECTORS_PER_BLOCK, &bytes)?;
        self.dev_flush()?;
        self.dev_write(0, &bytes)?;
        self.dev_flush()?;
        self.sb = sb.clone();
        Ok(())
    }

    /// Reads the whole bitmap into memory.
    fn load_bitmap(&mut self) -> Result<(), FsError> {
        let blocks = self.sb.bitmap_blocks;
        let mut bytes = vec![0u8; blocks as usize * BLOCK];
        self.read_blocks(self.sb.bitmap_start, &mut bytes)?;
        self.bitmap = Bitmap::new(bytes, self.sb.block_count);
        Ok(())
    }

    /// Writes the bitmap blocks that changed, then flushes.
    pub(crate) fn write_bitmap(&mut self) -> Result<(), FsError> {
        let start = self.sb.bitmap_start;
        let dirty: Vec<(usize, Vec<u8>)> =
            self.bitmap.dirty_blocks().map(|(i, b)| (i, b.to_vec())).collect();
        if dirty.is_empty() {
            return Ok(());
        }
        for (i, bytes) in dirty {
            self.write_blocks(start + i as u64, &bytes)?;
        }
        self.dev_flush()?;
        self.bitmap.clear_dirty();
        Ok(())
    }

    /// Reads the inode table's extent list and checks it.
    fn load_table(&mut self) -> Result<(), MountError> {
        let table_inode = self.sb.table.clone();
        if table_inode.kind != TYPE_FILE {
            return Err(MountError::Damaged("the inode table is not a file"));
        }
        let loaded = self
            .load_extents(&table_inode)
            .map_err(|e| mount_error(e, "the inode table's extents are unreadable"))?;
        // The table must have no gaps (every inode slot must be somewhere on the disk) and be
        // exactly as long as the superblock's inode count says. That also bounds `inode_count`
        // by the number of blocks really there, so a lying superblock cannot make a check loop
        // over billions of imaginary inodes.
        let mut next = 0;
        for e in loaded.extents.iter() {
            if e.file_block != next {
                return Err(MountError::Damaged("the inode table has a gap"));
            }
            next = e.file_end();
        }
        if table_inode.size != u64::from(self.sb.inode_count) * INODE_SIZE as u64
            || next != table_inode.size.div_ceil(BLOCK_U64)
        {
            return Err(MountError::Damaged("the inode table is not the size the superblock says"));
        }
        self.table = loaded;
        Ok(())
    }

    /// Where inode `n` is: its disk block and the byte offset within it.
    pub(crate) fn inode_place(&self, n: u32) -> Result<(u64, usize), FsError> {
        if n == 0 || n >= self.sb.inode_count {
            return Err(FsError::Damaged);
        }
        let block = lookup(&self.table.extents, u64::from(n / INODES_PER_BLOCK));
        let block = block.ok_or(FsError::Damaged)?;
        Ok((block, (n % INODES_PER_BLOCK) as usize * INODE_SIZE))
    }

    /// Reads inode `n`; with `verify` an in-use inode must have a good checksum.
    pub(crate) fn read_inode(&mut self, n: u32, verify: bool) -> Result<Inode, FsError> {
        let (block, off) = self.inode_place(n)?;
        // An inode never straddles a sector, so one sector read is enough.
        let mut sector = [0u8; SECTOR];
        let at = block * SECTORS_PER_BLOCK + (off / SECTOR) as u64;
        self.dev_read(at, &mut sector)?;
        let start = off % SECTOR;
        Inode::parse(&sector[start..start + INODE_SIZE], verify)
    }

    /// Writes inode `n` in place. This is a single sector write (read the sector, change the
    /// inode's 256 bytes in it, write it back), which is why a change to an inode is the
    /// moment a change to a file takes effect: it happens completely or not at all.
    pub(crate) fn write_inode(&mut self, n: u32, inode: &Inode) -> Result<(), FsError> {
        self.writable()?;
        let (block, off) = self.inode_place(n)?;
        let mut sector = [0u8; SECTOR];
        let at = block * SECTORS_PER_BLOCK + (off / SECTOR) as u64;
        self.dev_read(at, &mut sector)?;
        let start = off % SECTOR;
        sector[start..start + INODE_SIZE].copy_from_slice(&inode.to_bytes());
        self.dev_write(at, &sector)?;
        if self.cache.as_ref().is_some_and(|c| c.0 == n) {
            self.cache = None;
        }
        Ok(())
    }

    /// Frees blocks in the in-memory bitmap. A block that is already free means two things
    /// claimed it (a damaged volume); there is nothing useful to do about that here, and the
    /// check reports it.
    pub(crate) fn release(&mut self, runs: &[Run]) {
        for r in crate::runs::merge_runs(runs.to_vec()) {
            let _ = self.bitmap.mark_free(r);
        }
    }

    /// Frees single blocks (an old extent tree's).
    pub(crate) fn release_blocks(&mut self, blocks: &[u64]) {
        let runs: Vec<Run> = blocks.iter().map(|&b| Run { start: b, len: 1 }).collect();
        self.release(&runs);
    }
}

/// Builds a `NodeId` from an inode number and its generation (note 0016, section 2).
pub(crate) fn node_id(inode: u32, generation: u32) -> NodeId {
    u64::from(generation) << 32 | u64::from(inode)
}

/// Turns an error met while mounting into a [`MountError`] saying `what` was wrong.
fn mount_error(e: FsError, what: &'static str) -> MountError {
    match e {
        FsError::Disk => MountError::Disk,
        _ => MountError::Damaged(what),
    }
}

/// Checks that what a superblock says about the volume's layout is possible on this device.
fn check_geometry(sb: &Superblock, device_blocks: u64) -> Result<(), MountError> {
    let bad = MountError::Damaged("the superblock describes a layout that is not possible");
    if sb.block_count < MIN_BLOCKS || sb.block_count > device_blocks {
        return Err(bad);
    }
    if sb.bitmap_start != 1
        || sb.bitmap_blocks != sb.block_count.div_ceil(BITS_PER_BITMAP_BLOCK)
        || sb.data_start != 1 + sb.bitmap_blocks
        || sb.data_start + 2 > sb.block_count
    {
        return Err(bad);
    }
    if sb.inode_count < 2 || sb.root_inode == 0 || sb.root_inode >= sb.inode_count {
        return Err(bad);
    }
    Ok(())
}
