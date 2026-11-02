// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Checking a volume, and repairing it (ROCKFS.md, "Checking a volume").
//!
//! A *check* looks at every structure on the volume and compares what it finds with the rules
//! of the format. A *repair* also fixes what it can. It is how a volume that was not shut down
//! cleanly is made safe to use again: RockFS writes the block bitmap lazily, so after a power cut
//! the bitmap on the disk may be behind, and a save that was interrupted may have left blocks or
//! an empty item that belong to nothing. None of that loses data, and the check puts it right.
//!
//! The check works in three passes:
//! 1. **Scan**: read every inode, check its checksum and extent tree, and work out which blocks
//!    are really in use and by whom.
//! 2. **Walk**: start at the root folder and follow every folder, checking that each name points
//!    at a good inode of the right type whose `parent` agrees, and that no two names in a
//!    folder count as the same.
//! 3. **Compare** the blocks really in use with the bitmap.
//!
//! A repair then rebuilds the bitmap, removes the names that point at nothing, and moves any
//! inode that no folder points to into `/lost+found` (empty ones are just removed).

use crate::bitmap::Bitmap;
use crate::dirs::folder_shape_ok;
use crate::error::Problem;
use crate::extents::lookup;
use crate::folder::Entries;
use crate::inode::Inode;
use crate::layout::*;
use crate::names::valid_stored_name;
use crate::runs::Run;
use crate::tree::Loaded;
use crate::volume::RockFs;
use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use pbr_fs::{BlockDevice, FsError};

/// What the scan found at each inode number.
enum Slot {
    /// Not in use (or inode 0).
    Free,
    /// In use, but damaged (bad checksum, unknown type or broken extent tree).
    Bad,
    /// In use and good.
    Live { kind: u16, parent: u32, size: u64, folder: Option<Loaded> },
}

/// The result of the scan.
struct Scan {
    /// The blocks really in use.
    used: Bitmap,
    slots: Vec<Slot>,
    /// Damaged inodes and the generation they had, so a repair frees them with a newer one.
    bad: Vec<(u32, u32)>,
}

/// A repair to make once the walk is done.
enum Fix {
    /// Remove the name (exactly as stored) for this inode from this folder.
    Unlink { folder: u32, inode: u32, name: Vec<u8> },
    /// Record that this inode is in this folder.
    Parent { inode: u32, folder: u32 },
    /// Correct the type recorded in this folder's entry.
    Retype { folder: u32, name: Vec<u8>, inode: u32, kind: u8 },
}

/// Problems found so far, each with whether a repair can fix it.
struct Report {
    list: Vec<(Problem, bool)>,
}

impl Report {
    fn add(&mut self, text: String, harmless: bool, fixable: bool) {
        self.list.push((Problem { text, harmless, fixed: false }, fixable));
    }
}

impl<D: BlockDevice> RockFs<D> {
    /// Checks the volume, and with `repair` fixes what it can. Returns what it found; with
    /// `repair`, each problem says whether it was fixed.
    ///
    /// Changes not yet committed are committed first (a check compares the disk with what files
    /// use, and uncommitted blocks are in the bitmap but not yet in any file). `ReadOnly` if
    /// `repair` is asked for on a read-only mount, and `Damaged` if the volume is too damaged
    /// to repair (for example its root folder is gone).
    pub fn check(&mut self, repair: bool) -> Result<Vec<Problem>, FsError> {
        if repair {
            self.writable()?;
        }
        if !self.read_only {
            self.commit_all()?;
        }
        self.check_inner(repair)
    }

    /// The check itself (see [`RockFs::check`]); also run at mount, when the volume wasn't shut
    /// down cleanly.
    pub(crate) fn check_inner(&mut self, repair: bool) -> Result<Vec<Problem>, FsError> {
        let mut report = Report { list: Vec::new() };
        if self.main_damaged {
            report.add("the main superblock was damaged; the backup was used".into(), false, true);
        }
        let scan = self.scan(&mut report, false)?;
        let root = self.sb.root_inode;
        let root_ok =
            matches!(scan.slots.get(root as usize), Some(Slot::Live { kind: TYPE_FOLDER, .. }));
        if !root_ok {
            report.add("the top-level folder is missing or damaged".into(), false, false);
            return if repair { Err(FsError::Damaged) } else { Ok(finish(report, false)) };
        }

        // Walk the folders from the root.
        let count = scan.slots.len();
        let mut seen = vec![false; count];
        let mut fixes: Vec<Fix> = Vec::new();
        seen[root as usize] = true;
        self.walk(&scan, &mut seen, vec![root], &mut report, &mut fixes)?;
        // Anything in use that no folder reached belongs to no folder. A folder that nothing
        // points at is walked too, so its contents stay with it instead of each being
        // treated as lost.
        let mut orphans: Vec<u32> = Vec::new();
        for n in 1..count {
            if matches!(scan.slots[n], Slot::Live { .. }) && !seen[n] {
                orphans.push(n as u32);
                seen[n] = true;
                self.walk(&scan, &mut seen, vec![n as u32], &mut report, &mut fixes)?;
            }
        }
        if !orphans.is_empty() {
            report.add(format!("{} item(s) belong to no folder", orphans.len()), true, true);
        }

        // Compare the blocks really in use with the bitmap. After an unclean stop the bitmap
        // on the disk is expected to be behind, so that is not a sign of damage then.
        let behind = self.unclean_at_mount;
        let mut wrong = [Vec::<Run>::new(), Vec::new()];
        for b in 0..self.sb.block_count {
            let (really, marked) = (scan.used.is_used(b), self.bitmap.is_used(b));
            if really != marked {
                let list = &mut wrong[usize::from(really)];
                match list.last_mut() {
                    Some(r) if r.end() == b => r.len += 1,
                    _ => list.push(Run { start: b, len: 1 }),
                }
            }
        }
        for (really, runs) in wrong.iter().enumerate() {
            for r in runs {
                let (what, harmless) = match (really == 1, r.len == 1) {
                    (true, true) => {
                        (format!("block {} is in use but marked free", r.start), behind)
                    }
                    (true, false) => (
                        format!("blocks {}-{} are in use but marked free", r.start, r.end() - 1),
                        behind,
                    ),
                    (false, true) => {
                        (format!("block {} is marked used but belongs to nothing", r.start), true)
                    }
                    (false, false) => (
                        format!(
                            "blocks {}-{} are marked used but belong to nothing",
                            r.start,
                            r.end() - 1
                        ),
                        true,
                    ),
                };
                report.add(what, harmless, true);
            }
        }

        // With nothing wrong there is nothing to write, so a repair of a sound volume is free.
        if repair && !report.list.is_empty() {
            self.repair(scan, &orphans, fixes)?;
        }
        Ok(finish(report, repair))
    }

    /// Pass 1: reads every inode and works out which blocks are in use.
    fn scan(&mut self, report: &mut Report, quiet: bool) -> Result<Scan, FsError> {
        let blocks = self.sb.block_count;
        let mut used = Bitmap::empty(blocks);
        let mut slots: Vec<Slot> = Vec::new();
        let mut bad = Vec::new();
        // The superblock, the bitmap, the backup superblock.
        for run in [Run { start: 0, len: self.sb.data_start }, Run { start: blocks - 1, len: 1 }] {
            let _ = used.mark_used(run);
        }
        let table = self.table.clone();
        let runs: Vec<Run> = table.extents.iter().map(|e| e.run()).collect();
        claim(&mut used, &runs, "the inode table", report, quiet);
        claim_blocks(&mut used, &table.tree, "the inode table", report, quiet);

        let count = self.sb.inode_count;
        slots.push(Slot::Free); // inode 0 is never used
        let mut n = 1u32;
        while n < count {
            let first = n - n % INODES_PER_BLOCK;
            let disk = lookup(&self.table.extents, u64::from(first / INODES_PER_BLOCK))
                .ok_or(FsError::Damaged)?;
            let block = self.read_block(disk)?;
            for i in n % INODES_PER_BLOCK..INODES_PER_BLOCK {
                let slot = first + i;
                if slot >= count {
                    break;
                }
                let raw = &block[i as usize * INODE_SIZE..][..INODE_SIZE];
                let generation = u32_at(raw, 32);
                let who = format!("inode {slot}");
                let kind = u16_at(raw, 0);
                if kind == TYPE_FREE {
                    slots.push(Slot::Free);
                    continue;
                }
                let inode = match Inode::parse(raw, true) {
                    Ok(inode) => inode,
                    Err(_) => {
                        if !quiet {
                            report.add(format!("{who} is damaged (bad checksum)"), false, true);
                        }
                        bad.push((slot, generation));
                        slots.push(Slot::Bad);
                        continue;
                    }
                };
                if kind != TYPE_FILE && kind != TYPE_FOLDER {
                    if !quiet {
                        report.add(format!("{who} has an unknown type {kind}"), false, true);
                    }
                    bad.push((slot, generation));
                    slots.push(Slot::Bad);
                    continue;
                }
                match self.load_extents(&inode) {
                    Ok(loaded) => {
                        let runs: Vec<Run> = loaded.extents.iter().map(|e| e.run()).collect();
                        claim(&mut used, &runs, &who, report, quiet);
                        claim_blocks(&mut used, &loaded.tree, &who, report, quiet);
                        let folder = (kind == TYPE_FOLDER).then_some(loaded);
                        slots.push(Slot::Live {
                            kind,
                            parent: inode.parent,
                            size: inode.size,
                            folder,
                        });
                    }
                    Err(FsError::Disk) => return Err(FsError::Disk),
                    Err(_) => {
                        if !quiet {
                            report.add(format!("{who} has a damaged extent tree"), false, true);
                        }
                        bad.push((slot, generation));
                        slots.push(Slot::Bad);
                    }
                }
            }
            n = first + INODES_PER_BLOCK;
        }
        Ok(Scan { used, slots, bad })
    }

    /// Pass 2: walks the folders in `queue` and everything below them, checking every name.
    fn walk(
        &mut self,
        scan: &Scan,
        seen: &mut [bool],
        mut queue: Vec<u32>,
        report: &mut Report,
        fixes: &mut Vec<Fix>,
    ) -> Result<(), FsError> {
        let mut next = 0;
        while next < queue.len() {
            let dir = queue[next];
            next += 1;
            let Some(Slot::Live { size, folder: Some(view), .. }) = scan.slots.get(dir as usize)
            else {
                continue;
            };
            if folder_shape_ok(view, *size).is_err() {
                report.add(format!("folder {dir} is damaged"), false, false);
                continue;
            }
            let mut keys: BTreeSet<Vec<u8>> = BTreeSet::new();
            'blocks: for k in 0..size / BLOCK_U64 {
                let block = self.read_folder_block(view, k)?;
                for entry in Entries::new(&block[..]) {
                    let Ok(entry) = entry else {
                        report.add(format!("folder {dir} is damaged"), false, false);
                        break 'blocks;
                    };
                    if entry.inode == 0 {
                        continue;
                    }
                    let name = entry.name(&block[..]).to_vec();
                    let shown = String::from_utf8_lossy(&name).into_owned();
                    let where_ = format!("folder {dir}, \"{shown}\"");
                    let child = entry.inode;
                    let unlink = Fix::Unlink { folder: dir, inode: child, name: name.clone() };
                    if !valid_stored_name(&name) {
                        report.add(format!("{where_} is not a valid name"), false, true);
                        fixes.push(unlink);
                        continue;
                    }
                    let Some(Slot::Live { kind, parent, folder, .. }) =
                        scan.slots.get(child as usize).filter(|_| child != 0)
                    else {
                        report.add(
                            format!("{where_} points at a missing or damaged item (inode {child})"),
                            false,
                            true,
                        );
                        fixes.push(unlink);
                        continue;
                    };
                    // An item named in two folders is one a move across folders was moving
                    // when the machine stopped. The name in the folder its `parent` names is
                    // the one to keep: a rename writes `parent` at the point the move counts
                    // as done (see `rename` in ops.rs).
                    let says = *parent;
                    let elsewhere = says != dir && !seen[child as usize] && {
                        match scan.slots.get(says as usize) {
                            Some(Slot::Live { size, folder: Some(view), .. }) => {
                                self.dir_names(view, *size, child)?
                            }
                            _ => false,
                        }
                    };
                    if elsewhere || seen[child as usize] {
                        report.add(
                            format!("{where_} is a second name for inode {child}"),
                            false,
                            true,
                        );
                        fixes.push(unlink);
                        continue;
                    }
                    let key: Vec<u8> = name.to_ascii_lowercase();
                    if !keys.insert(key) {
                        report.add(
                            format!("folder {dir} has two names that count as \"{shown}\""),
                            false,
                            false,
                        );
                    }
                    seen[child as usize] = true;
                    if u16::from(entry.kind) != *kind {
                        report.add(
                            format!("{where_} is recorded with the wrong type"),
                            false,
                            true,
                        );
                        fixes.push(Fix::Retype {
                            folder: dir,
                            name: name.clone(),
                            inode: child,
                            kind: *kind as u8,
                        });
                    }
                    if *parent != dir {
                        report.add(
                            format!(
                                "inode {child} says its folder is {parent}, but it is in {dir}"
                            ),
                            false,
                            true,
                        );
                        fixes.push(Fix::Parent { inode: child, folder: dir });
                    }
                    if folder.is_some() {
                        queue.push(child);
                    }
                }
            }
        }
        Ok(())
    }

    /// Makes the repairs: the bitmap first (so nothing is allocated over a block in use), then
    /// damaged inodes, bad names, orphans, and finally the bitmap and hints again.
    fn repair(&mut self, scan: Scan, orphans: &[u32], fixes: Vec<Fix>) -> Result<(), FsError> {
        self.bitmap.replace(scan.used.bytes().to_vec());
        self.cache = None;
        self.inode_hint = 2;
        // Damaged inodes are freed, with a newer generation so old NodeIds stay Gone.
        for &(n, generation) in &scan.bad {
            self.write_inode(n, &Inode::free(generation.wrapping_add(1)))?;
        }
        self.dev_flush()?;
        for fix in fixes {
            match fix {
                Fix::Unlink { folder, inode, name } => self.dir_delete(folder, inode, &name)?,
                Fix::Parent { inode, folder } => self.set_parent(inode, folder)?,
                Fix::Retype { folder, name, inode, kind } => {
                    self.dir_repoint(folder, &name, inode, kind, true)?
                }
            }
        }
        self.repair_orphans(&scan, orphans)?;
        // The repairs changed what is in use, so work the bitmap out again from scratch.
        let mut quiet = Report { list: Vec::new() };
        let after = self.scan(&mut quiet, true)?;
        self.bitmap.replace(after.used.bytes().to_vec());
        let free = after.slots.iter().skip(2).filter(|s| matches!(s, Slot::Free)).count();
        self.sb.free_inodes = free as u32;
        self.sb.free_blocks = self.bitmap.free();
        self.write_bitmap()?;
        let sb = self.sb.clone();
        self.write_superblocks(&sb)?;
        self.main_damaged = false;
        Ok(())
    }

    /// Frees the empty orphans and moves the rest into `/lost+found`, named `#` and the inode
    /// number.
    fn repair_orphans(&mut self, scan: &Scan, orphans: &[u32]) -> Result<(), FsError> {
        let mut keep: Vec<(u32, u16)> = Vec::new();
        for &n in orphans {
            let Some(Slot::Live { kind, size, .. }) = scan.slots.get(n as usize) else { continue };
            if *size == 0 {
                // An empty leftover of a save that was cut short: no reason to keep it.
                let inode = self.read_inode(n, true)?;
                self.free_inode(n, &inode)?;
            } else {
                keep.push((n, *kind));
            }
        }
        if keep.is_empty() {
            return Ok(());
        }
        let root = self.sb.root_inode;
        let root_inode = self.read_inode(root, true)?;
        let lost = match self.dir_find(root, &root_inode, b"lost+found")? {
            Some(found) => {
                let (n, inode) = self.read_child(&found)?;
                if inode.kind != TYPE_FOLDER {
                    return Ok(()); // a file is in the way; leave the orphans where they are
                }
                n
            }
            None => {
                let (n, _) = self.alloc_inode(TYPE_FOLDER, root)?;
                self.dir_insert(root, b"lost+found", n, TYPE_FOLDER as u8, true)?;
                n
            }
        };
        for (n, kind) in keep {
            let name = format!("#{n}");
            self.dir_insert(lost, name.as_bytes(), n, kind as u8, true)?;
            self.set_parent(n, lost)?;
        }
        Ok(())
    }
}

/// Turns the report into the list the caller sees, marking what a repair fixed.
fn finish(report: Report, repaired: bool) -> Vec<Problem> {
    report
        .list
        .into_iter()
        .map(|(mut p, fixable)| {
            p.fixed = repaired && fixable;
            p
        })
        .collect()
}

/// Marks `runs` in use in `used` on behalf of `who`, reporting any block already claimed.
fn claim(used: &mut Bitmap, runs: &[Run], who: &str, report: &mut Report, quiet: bool) {
    for &run in runs {
        if used.mark_used(run) {
            continue;
        }
        // Some block of the run is outside the volume or already taken: find which.
        for b in run.start..run.end() {
            if !used.mark_used(Run { start: b, len: 1 }) && !quiet {
                report.add(format!("block {b} is claimed twice (again by {who})"), false, false);
            }
        }
    }
}

/// Like [`claim`], for single blocks.
fn claim_blocks(used: &mut Bitmap, blocks: &[u64], who: &str, report: &mut Report, quiet: bool) {
    let runs: Vec<Run> = blocks.iter().map(|&b| Run { start: b, len: 1 }).collect();
    claim(used, &runs, &format!("{who} (extent tree)"), report, quiet);
}
