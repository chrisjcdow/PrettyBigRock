// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! A mounted FAT32 volume: mounting and unmounting it, and the table of nodes.
//!
//! To **mount** a volume is to open it for use: check its boot sector, find out how much of it is
//! free, and mark it "in use" on the disk. To **unmount** is the reverse, and the last thing it
//! does is mark it "shut down cleanly". The mark is bit 27 of the FAT's second entry; Windows
//! and Linux set and clear it the same way, and when it is found clear (the last user never
//! unmounted), they suggest running a checker. We cannot repair FAT volumes in M5, so such a
//! volume is used as it is and the fact is reported in [`Info`] (note 0016, section 3).
//!
//! FAT has no inode numbers: a file is just an entry in a folder. [`Fat`] therefore keeps its
//! own table of *nodes*, one for each file or folder that has been looked up or created. A node
//! gets a [`NodeId`] from a counter that only goes up, so an id is never used for a second
//! file. A node records where its entry is, and, for a file, the changes not yet committed.

use crate::boot::{FsInfo, Geometry};
use crate::chain::Chain;
use crate::error::{Info, MountError};
use crate::names::label_text;
use crate::table::{CLEAN_BIT, FatTable};
use crate::volume::Volume;
use alloc::collections::BTreeMap;
use alloc::string::String;
use pbr_fs::{BlockDevice, FsError, Kind, NodeId, SECTOR};

/// The root folder's id. It has no entry in any folder.
pub(crate) const ROOT: NodeId = 1;

/// A file or folder that has been looked up or created.
pub(crate) struct Node {
    /// File or folder.
    pub kind: Kind,
    /// The folder it is in (0 for the root).
    pub parent: NodeId,
    /// The slot of its short entry in that folder.
    pub slot: u32,
    /// Its clusters. For a file with changes not yet committed this is ahead of the disk.
    pub chain: Chain,
    /// A file's size, including changes not yet committed (0 for a folder).
    pub size: u32,
    /// When it was created (seconds since 1970; 0 if not recorded).
    pub created: u64,
    /// When it was last changed, including changes not yet committed.
    pub modified: u64,
    /// Whether it is marked hidden.
    pub hidden: bool,
    /// Whether its entry on the disk is behind (new size, first cluster or time).
    pub dirty: bool,
    /// Whether its whole chain has been walked and found to be a proper one (no loop, nothing
    /// outside the volume). A chain is checked before the first read or write of the file, so
    /// that a loop is reported as damage and not read as if it were data.
    pub verified: bool,
    /// Whether its chain may be longer than its size needs, because it was cut and the extra
    /// clusters have not been freed yet (that waits for the commit).
    pub trim: bool,
}

/// A mounted FAT32 volume on a block device `D`.
pub struct Fat<D: BlockDevice> {
    /// The disk, geometry and FAT.
    pub(crate) vol: Volume<D>,
    /// Seconds since 1970 UTC; called whenever something is created or changed.
    pub(crate) clock: fn() -> u64,
    /// Whether every change is refused.
    pub(crate) read_only: bool,
    /// Set when a write to the disk failed. The picture in memory and the disk may then
    /// disagree, so every later change is refused and the volume is not marked clean.
    pub(crate) poisoned: bool,
    /// Every node by id.
    pub(crate) nodes: BTreeMap<NodeId, Node>,
    /// The node of each entry that has one: (folder, slot) to id. So that looking up the same
    /// file twice gives the same id.
    pub(crate) by_entry: BTreeMap<(NodeId, u32), NodeId>,
    /// The id the next new node gets.
    pub(crate) next_id: NodeId,
    /// The volume label.
    label: String,
    /// Whether the volume was cleanly unmounted before this mount.
    clean_at_mount: bool,
    /// Whether the FSInfo hints were believed.
    used_fsinfo: bool,
}

/// What a failure while mounting means.
fn mount_error(e: FsError, why: &'static str) -> MountError {
    match e {
        FsError::Disk => MountError::Disk,
        _ => MountError::Damaged(why),
    }
}

impl<D: BlockDevice> Fat<D> {
    /// Mounts the FAT32 volume on `dev` (sector 0 of `dev` is the volume's first sector).
    ///
    /// `clock` gives the time in seconds since 1970 UTC. The volume is mounted read-only if
    /// `read_only` is set or the device is read-only. Otherwise its "cleanly unmounted" flag is
    /// cleared on the disk at once, and set again by [`pbr_fs::Filesystem::unmount`].
    pub fn mount(dev: D, clock: fn() -> u64, read_only: bool) -> Result<Fat<D>, MountError> {
        let mut dev = dev;
        if dev.sectors() == 0 {
            return Err(MountError::NotFat);
        }
        let mut boot = [0u8; SECTOR];
        dev.read(0, &mut boot).map_err(|_| MountError::Disk)?;
        let geo = Geometry::parse(&boot, dev.sectors())?;
        let read_only = read_only || dev.read_only();
        let table = FatTable::new(&geo);
        let mut vol = Volume { dev, geo, table, free: 0, next_free: 2 };

        // The root folder must at least start a proper chain.
        vol.follow(vol.geo.root)
            .map_err(|e| mount_error(e, "the root folder's cluster chain is broken"))?;
        let entry1 =
            vol.table.get(&mut vol.dev, 1).map_err(|e| mount_error(e, "the FAT cannot be read"))?;
        let clean = entry1 & CLEAN_BIT != 0;

        // The FSInfo hints are only believed if the last user shut the volume down properly: after
        // a crash they are likely stale, and a stale but believable count would be wrong for good.
        let mut hints = FsInfo { free: None, next_free: None };
        if clean && let Some(sector) = vol.geo.fsinfo {
            let mut raw = [0u8; SECTOR];
            vol.dev.read(u64::from(sector), &mut raw).map_err(|_| MountError::Disk)?;
            if let Some(h) = FsInfo::parse(&raw, &vol.geo) {
                hints = h;
            }
        }
        vol.free = match hints.free {
            Some(free) => free,
            None => vol
                .table
                .count_free(&mut vol.dev)
                .map_err(|e| mount_error(e, "the FAT cannot be read"))?,
        };
        vol.next_free = hints.next_free.unwrap_or(2);

        let label = match vol.root_label() {
            Ok(Some(l)) => label_text(&l),
            // No label entry, or a root folder we cannot read (which will show up when it is
            // used): fall back to the boot sector's.
            _ => vol
                .geo
                .label
                .filter(|l| l != b"NO NAME    ")
                .map_or_else(String::new, |l| label_text(&l)),
        };

        if !read_only {
            vol.table
                .set(&mut vol.dev, 1, entry1 & !CLEAN_BIT)
                .and_then(|()| vol.flush())
                .map_err(|e| mount_error(e, "the FAT cannot be written"))?;
        }

        let root = Node {
            kind: Kind::Folder,
            parent: 0,
            slot: 0,
            chain: Chain::new(vol.geo.root),
            size: 0,
            created: 0,
            modified: 0,
            hidden: false,
            dirty: false,
            verified: true,
            trim: false,
        };
        let mut nodes = BTreeMap::new();
        nodes.insert(ROOT, root);
        Ok(Fat {
            vol,
            clock,
            read_only,
            poisoned: false,
            nodes,
            by_entry: BTreeMap::new(),
            next_id: ROOT + 1,
            label,
            clean_at_mount: clean,
            used_fsinfo: hints.free.is_some(),
        })
    }

    /// Facts about the volume.
    pub fn info(&self) -> Info {
        Info {
            label: self.label.clone(),
            volume_id: self.vol.geo.volume_id,
            cluster_bytes: self.vol.geo.cluster_bytes,
            cluster_count: self.vol.geo.clusters,
            free_clusters: self.vol.free,
            fat_copies: if self.vol.geo.mirror { self.vol.geo.fats } else { 1 },
            clean_at_mount: self.clean_at_mount,
            used_fsinfo: self.used_fsinfo,
            read_only: self.read_only,
        }
    }

    /// Gives the disk back, without writing anything. Unmount first
    /// ([`pbr_fs::Filesystem::unmount`]) to leave the volume clean; this alone leaves it as if
    /// the machine had stopped now, which is what the tests use to look at a crashed volume.
    pub fn into_device(self) -> D {
        self.vol.dev
    }

    /// A look at the disk, for tests.
    pub fn device(&self) -> &D {
        &self.vol.dev
    }

    /// The disk, for tests that need to change how it behaves.
    pub fn device_mut(&mut self) -> &mut D {
        &mut self.vol.dev
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

    /// Passes `result` on, first noting a disk failure: after one, the disk and our picture of
    /// it may disagree, so no more changes are made.
    pub(crate) fn track<T>(&mut self, result: Result<T, FsError>) -> Result<T, FsError> {
        if matches!(result, Err(FsError::Disk)) {
            self.poisoned = true;
        }
        result
    }

    /// Writes the free count and the search hint to the FSInfo sector, if the volume has one.
    pub(crate) fn write_fsinfo(&mut self) -> Result<(), FsError> {
        let Some(sector) = self.vol.geo.fsinfo else { return Ok(()) };
        let mut raw = [0u8; SECTOR];
        self.vol.dev.read(u64::from(sector), &mut raw)?;
        // A sector without the signatures is not an FSInfo sector, whatever the boot sector
        // says: leave it alone.
        if FsInfo::has_signatures(&raw) {
            FsInfo::store(&mut raw, self.vol.free, self.vol.next_free);
            self.vol.dev.write(u64::from(sector), &raw)?;
        }
        Ok(())
    }

    /// Sets the "cleanly unmounted" flag, on the disk. The caller has flushed everything else.
    pub(crate) fn mark_clean(&mut self) -> Result<(), FsError> {
        let v = &mut self.vol;
        let entry1 = v.table.get(&mut v.dev, 1)?;
        v.table.set(&mut v.dev, 1, entry1 | CLEAN_BIT)?;
        v.flush()
    }
}
