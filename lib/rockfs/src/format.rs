// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Making a new, empty RockFS volume (ROCKFS.md, "Formatting a new volume").

use crate::bitmap::Bitmap;
use crate::extents::Extent;
use crate::inode::{Inode, Top};
use crate::layout::*;
use crate::runs::Run;
use crate::superblock::Superblock;
use alloc::vec;
use alloc::vec::Vec;
use pbr_fs::{BlockDevice, FsError, SECTOR};

/// Formats the whole of `dev` as an empty RockFS volume called `label`.
///
/// `uuid` is the volume's ID, which the caller chooses (it only has to tell volumes apart);
/// `now` is the time to record as its creation time. Whatever was on the device is lost. The
/// volume is `dev.sectors() / 8` blocks of 4 KiB; a leftover partial block at the end is unused.
///
/// `BadArgument` if the device is smaller than 256 KiB, `ReadOnly` if it cannot be written.
pub fn format<D: BlockDevice>(
    dev: &mut D,
    label: &str,
    uuid: [u8; 16],
    now: u64,
) -> Result<(), FsError> {
    if dev.read_only() {
        return Err(FsError::ReadOnly);
    }
    let block_count = dev.sectors() / SECTORS_PER_BLOCK;
    if block_count < MIN_BLOCKS {
        return Err(FsError::BadArgument);
    }
    let bitmap_blocks = block_count.div_ceil(BITS_PER_BITMAP_BLOCK);
    let data_start = 1 + bitmap_blocks;
    // The inode table starts as one block, the first data block.
    let table_block = data_start;

    // The bitmap: the superblock, the bitmap itself, the table's block and the backup
    // superblock are in use; everything else is free.
    let mut bitmap = Bitmap::empty(block_count);
    for run in [Run { start: 0, len: data_start + 1 }, Run { start: block_count - 1, len: 1 }] {
        let marked = bitmap.mark_used(run);
        debug_assert!(marked);
    }

    // Inode 0 is never used; inode 1 is the empty root folder, its own parent.
    let mut table = vec![0u8; BLOCK];
    let root = Inode {
        kind: TYPE_FOLDER,
        flags: 0,
        parent: 1,
        size: 0,
        created: now,
        modified: now,
        generation: 0,
        depth: 0,
        top: Top::Extents(Vec::new()),
    };
    table[INODE_SIZE..2 * INODE_SIZE].copy_from_slice(&root.to_bytes());

    let table_inode = Inode {
        kind: TYPE_FILE,
        flags: 0,
        parent: 0,
        size: BLOCK_U64,
        created: now,
        modified: now,
        generation: 0,
        depth: 0,
        top: Top::Extents(vec![Extent { file_block: 0, disk_block: table_block, length: 1 }]),
    };
    let sb = Superblock {
        version: 1,
        block_shift: 12,
        name_rule: 1,
        block_count,
        bitmap_start: 1,
        bitmap_blocks,
        data_start,
        free_blocks: bitmap.free(),
        reserve_blocks: (block_count / 100).max(16),
        inode_count: INODES_PER_BLOCK,
        free_inodes: INODES_PER_BLOCK - 2,
        root_inode: 1,
        state: 0,
        features_compat: 0,
        features_ro_compat: 0,
        features_incompat: 0,
        mount_count: 0,
        created: now,
        mounted: 0,
        uuid,
        label: Superblock::pack_label(label),
        table: table_inode,
    };

    // Everything except the superblocks first, then a flush, then the backup, then the main
    // copy, which readers look at first. So a device that stops half-way through is not taken
    // for a complete volume.
    let write = |dev: &mut D, block: u64, data: &[u8]| -> Result<(), FsError> {
        for (i, piece) in data.chunks(MAX_TRANSFER).enumerate() {
            let sector = block * SECTORS_PER_BLOCK + (i * MAX_TRANSFER / SECTOR) as u64;
            dev.write(sector, piece)?;
        }
        Ok(())
    };
    write(dev, 1, bitmap.bytes())?;
    write(dev, table_block, &table)?;
    dev.flush()?;
    write(dev, block_count - 1, &sb.block_bytes())?;
    dev.flush()?;
    write(dev, 0, &sb.block_bytes())?;
    dev.flush()?;
    Ok(())
}
