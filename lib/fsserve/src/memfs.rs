// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! A filesystem kept entirely in memory. It follows every rule of [`pbr_fs::Filesystem`]
//! (names compared with A-Z matching a-z, as RockFS does; removed nodes `Gone`; folders can't be
//! moved into themselves), so the server's rules can be tested on the host and in QEMU before
//! the real filesystems exist. Nothing is ever written to a disk, so `commit` only counts.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use pbr_fs::{Entry, Filesystem, FsError, Kind, NodeId, Space, Stat};

/// The biggest file it holds.
pub const MAX_FILE: u64 = 64 * 1024 * 1024;

struct Node {
    kind: Kind,
    parent: usize,
    data: Vec<u8>,
    /// For a folder: the comparison key of each name → (the name as given, the child's slot).
    children: BTreeMap<String, (String, usize)>,
    created: u64,
    modified: u64,
}

/// The filesystem.
pub struct MemFs {
    nodes: Vec<Option<Node>>,
    generations: Vec<u32>,
    /// Refuse every change.
    pub read_only: bool,
    /// How many times `commit` was called on a file with changes (for tests).
    pub commits: usize,
    /// Files changed since their last commit.
    dirty: Vec<usize>,
    /// The time, in seconds since 1970 UTC.
    pub clock: fn() -> u64,
    label: String,
}

fn no_clock() -> u64 {
    0
}

/// Name rule 1 (ROCKFS.md, "Names"): A-Z match a-z, everything else exactly.
fn key(name: &str) -> String {
    name.chars().map(|c| c.to_ascii_lowercase()).collect()
}

impl MemFs {
    /// An empty filesystem labelled `label`.
    pub fn new(label: &str) -> MemFs {
        let root = Node {
            kind: Kind::Folder,
            parent: 0,
            data: Vec::new(),
            children: BTreeMap::new(),
            created: 0,
            modified: 0,
        };
        MemFs {
            nodes: alloc::vec![Some(root)],
            generations: alloc::vec![0],
            read_only: false,
            commits: 0,
            dirty: Vec::new(),
            clock: no_clock,
            label: String::from(label),
        }
    }

    fn id(&self, slot: usize) -> NodeId {
        u64::from(self.generations[slot]) << 32 | slot as u64
    }

    fn slot(&self, id: NodeId) -> Result<usize, FsError> {
        let slot = (id & 0xFFFF_FFFF) as usize;
        match (self.nodes.get(slot), self.generations.get(slot)) {
            (Some(Some(_)), Some(&g)) if u64::from(g) == id >> 32 => Ok(slot),
            _ => Err(FsError::Gone),
        }
    }

    fn node(&self, id: NodeId) -> Result<&Node, FsError> {
        let s = self.slot(id)?;
        self.nodes[s].as_ref().ok_or(FsError::Gone)
    }

    fn node_mut(&mut self, id: NodeId) -> Result<&mut Node, FsError> {
        let s = self.slot(id)?;
        self.nodes[s].as_mut().ok_or(FsError::Gone)
    }

    fn writable(&self) -> Result<(), FsError> {
        if self.read_only { Err(FsError::ReadOnly) } else { Ok(()) }
    }

    fn folder(&self, id: NodeId) -> Result<&Node, FsError> {
        let n = self.node(id)?;
        if n.kind != Kind::Folder {
            return Err(FsError::NotAFolder);
        }
        Ok(n)
    }

    fn changed(&mut self, slot: usize) {
        if !self.dirty.contains(&slot) {
            self.dirty.push(slot);
        }
    }

    fn free(&mut self, slot: usize) {
        self.nodes[slot] = None;
        self.generations[slot] = self.generations[slot].wrapping_add(1);
        self.dirty.retain(|&s| s != slot);
    }
}

impl Filesystem for MemFs {
    fn root(&self) -> NodeId {
        self.id(0)
    }

    fn read_only(&self) -> bool {
        self.read_only
    }

    fn lookup(&mut self, folder: NodeId, name: &str) -> Result<NodeId, FsError> {
        let f = self.folder(folder)?;
        let &(_, slot) = f.children.get(&key(name)).ok_or(FsError::NotFound)?;
        Ok(self.id(slot))
    }

    fn stat(&mut self, node: NodeId) -> Result<Stat, FsError> {
        let n = self.node(node)?;
        let size = if n.kind == Kind::File { n.data.len() as u64 } else { 0 };
        Ok(Stat { kind: n.kind, size, created: n.created, modified: n.modified, hidden: false })
    }

    fn read(&mut self, node: NodeId, offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        let n = self.node(node)?;
        if n.kind != Kind::File {
            return Err(FsError::NotAFile);
        }
        let len = n.data.len() as u64;
        if offset >= len {
            return Ok(0);
        }
        let take = (len - offset).min(buf.len() as u64) as usize;
        let at = offset as usize;
        buf[..take].copy_from_slice(&n.data[at..at + take]);
        Ok(take)
    }

    fn write(&mut self, node: NodeId, offset: u64, data: &[u8]) -> Result<usize, FsError> {
        self.writable()?;
        let end = offset.checked_add(data.len() as u64).ok_or(FsError::TooBig)?;
        if end > MAX_FILE {
            return Err(FsError::TooBig);
        }
        let now = (self.clock)();
        let slot = self.slot(node)?;
        let n = self.node_mut(node)?;
        if n.kind != Kind::File {
            return Err(FsError::NotAFile);
        }
        if n.data.len() < end as usize {
            n.data.resize(end as usize, 0);
        }
        n.data[offset as usize..end as usize].copy_from_slice(data);
        n.modified = now;
        self.changed(slot);
        Ok(data.len())
    }

    fn set_size(&mut self, node: NodeId, size: u64) -> Result<(), FsError> {
        self.writable()?;
        if size > MAX_FILE {
            return Err(FsError::TooBig);
        }
        let now = (self.clock)();
        let slot = self.slot(node)?;
        let n = self.node_mut(node)?;
        if n.kind != Kind::File {
            return Err(FsError::NotAFile);
        }
        n.data.resize(size as usize, 0);
        n.modified = now;
        self.changed(slot);
        Ok(())
    }

    fn commit(&mut self, node: NodeId) -> Result<(), FsError> {
        let slot = self.slot(node)?;
        if let Some(i) = self.dirty.iter().position(|&s| s == slot) {
            self.dirty.remove(i);
            self.commits += 1;
        }
        Ok(())
    }

    fn list(
        &mut self,
        folder: NodeId,
        cursor: u64,
        out: &mut dyn FnMut(Entry) -> bool,
    ) -> Result<u64, FsError> {
        let f = self.folder(folder)?;
        // The cursor is 0 to start, then one more than the number of entries (in name order)
        // given out already, so it's never 0 part-way through.
        let skip = cursor.saturating_sub(1);
        let mut at = skip;
        for (name, slot) in f.children.values().skip(skip as usize) {
            let Some(c) = self.nodes[*slot].as_ref() else { continue };
            let size = if c.kind == Kind::File { c.data.len() as u64 } else { 0 };
            let e = Entry {
                name: name.clone(),
                kind: c.kind,
                size,
                modified: c.modified,
                hidden: false,
            };
            if !out(e) {
                return Ok(at + 1);
            }
            at += 1;
        }
        Ok(0)
    }

    fn create(&mut self, folder: NodeId, name: &str, kind: Kind) -> Result<NodeId, FsError> {
        self.writable()?;
        let k = key(name);
        let parent = self.slot(folder)?;
        if self.folder(folder)?.children.contains_key(&k) {
            return Err(FsError::AlreadyExists);
        }
        let now = (self.clock)();
        let node = Node {
            kind,
            parent,
            data: Vec::new(),
            children: BTreeMap::new(),
            created: now,
            modified: now,
        };
        let slot = match self.nodes.iter().position(Option::is_none) {
            Some(s) => {
                self.nodes[s] = Some(node);
                s
            }
            None => {
                self.nodes.push(Some(node));
                self.generations.push(0);
                self.nodes.len() - 1
            }
        };
        if let Some(p) = self.nodes[parent].as_mut() {
            p.children.insert(k, (String::from(name), slot));
            p.modified = now;
        }
        Ok(self.id(slot))
    }

    fn remove(&mut self, folder: NodeId, name: &str) -> Result<(), FsError> {
        self.writable()?;
        let k = key(name);
        let &(_, slot) = self.folder(folder)?.children.get(&k).ok_or(FsError::NotFound)?;
        if self.nodes[slot].as_ref().is_some_and(|n| !n.children.is_empty()) {
            return Err(FsError::FolderNotEmpty);
        }
        let parent = self.slot(folder)?;
        if let Some(p) = self.nodes[parent].as_mut() {
            p.children.remove(&k);
        }
        self.free(slot);
        Ok(())
    }

    fn rename(
        &mut self,
        from: NodeId,
        from_name: &str,
        to: NodeId,
        to_name: &str,
        replace: bool,
    ) -> Result<(), FsError> {
        self.writable()?;
        let (fk, tk) = (key(from_name), key(to_name));
        let &(_, moving) = self.folder(from)?.children.get(&fk).ok_or(FsError::NotFound)?;
        self.folder(to)?;
        let (from_slot, to_slot) = (self.slot(from)?, self.slot(to)?);
        // A folder can't go inside itself: walk up from the destination to the root.
        let mut up = to_slot;
        loop {
            if up == moving {
                return Err(FsError::BadArgument);
            }
            if up == 0 {
                break;
            }
            up = self.nodes[up].as_ref().map_or(0, |n| n.parent);
        }
        let existing = self.nodes[to_slot].as_ref().and_then(|n| n.children.get(&tk)).map(|x| x.1);
        match existing {
            Some(s) if s == moving => {} // the same node, perhaps a change of capitals
            Some(s) => {
                let is_file = self.nodes[s].as_ref().is_some_and(|n| n.kind == Kind::File);
                if !replace || !is_file {
                    return Err(FsError::AlreadyExists);
                }
                if let Some(t) = self.nodes[to_slot].as_mut() {
                    t.children.remove(&tk);
                }
                self.free(s);
            }
            None => {}
        }
        if let Some(f) = self.nodes[from_slot].as_mut() {
            f.children.remove(&fk);
        }
        if let Some(t) = self.nodes[to_slot].as_mut() {
            t.children.insert(tk, (String::from(to_name), moving));
        }
        if let Some(m) = self.nodes[moving].as_mut() {
            m.parent = to_slot;
        }
        Ok(())
    }

    fn space(&mut self) -> Result<Space, FsError> {
        let used: u64 = self.nodes.iter().flatten().map(|n| n.data.len() as u64).sum();
        let total = 256 * 1024 * 1024;
        Ok(Space {
            total,
            free: total.saturating_sub(used),
            label: self.label.clone(),
            filesystem: "memory",
        })
    }

    fn unmount(&mut self) -> Result<(), FsError> {
        self.commits += self.dirty.len();
        self.dirty.clear();
        Ok(())
    }
}
