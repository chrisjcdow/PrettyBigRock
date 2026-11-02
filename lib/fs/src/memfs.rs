// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! A filesystem kept in memory, for testing `lib/fsserve` and the file layer without a disk.
//! It follows the same rules a real filesystem must (node numbers never reused, names compared
//! as RockFS's rule 1 does: A-Z match a-z), and counts commits so tests can see them. Nothing
//! survives the program, so committing does nothing else.

use crate::{Entry, Filesystem, FsError, Kind, NodeId, Space, Stat, check_name};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// A file's or folder's contents.
struct Node {
    kind: Kind,
    parent: NodeId,
    data: Vec<u8>,
    /// For a folder: (the name's comparison key) → (the name as given, the node).
    children: BTreeMap<String, (String, NodeId)>,
    modified: u64,
}

/// The filesystem.
pub struct MemFs {
    nodes: BTreeMap<NodeId, Node>,
    next: NodeId,
    /// The most bytes it may hold in files.
    capacity: u64,
    used: u64,
    /// How many times `commit` found something to commit (for tests).
    pub commits: u64,
    /// Nodes changed since their last commit.
    dirty: BTreeMap<NodeId, ()>,
    read_only: bool,
}

/// The comparison key: A-Z as a-z, everything else as it is (RockFS's name rule 1).
fn key(name: &str) -> String {
    name.chars().map(|c| c.to_ascii_lowercase()).collect()
}

const ROOT: NodeId = 1;

impl MemFs {
    /// An empty filesystem that can hold `capacity` bytes of files.
    pub fn new(capacity: u64) -> MemFs {
        let mut nodes = BTreeMap::new();
        nodes.insert(
            ROOT,
            Node {
                kind: Kind::Folder,
                parent: ROOT,
                data: Vec::new(),
                children: BTreeMap::new(),
                modified: 0,
            },
        );
        MemFs {
            nodes,
            next: 2,
            capacity,
            used: 0,
            commits: 0,
            dirty: BTreeMap::new(),
            read_only: false,
        }
    }

    /// The same, but read-only.
    pub fn read_only(mut self) -> MemFs {
        self.read_only = true;
        self
    }

    fn node(&self, n: NodeId) -> Result<&Node, FsError> {
        self.nodes.get(&n).ok_or(FsError::Gone)
    }

    fn folder(&self, n: NodeId) -> Result<&Node, FsError> {
        let f = self.node(n)?;
        if f.kind == Kind::Folder { Ok(f) } else { Err(FsError::NotAFolder) }
    }

    fn file_mut(&mut self, n: NodeId) -> Result<&mut Node, FsError> {
        let f = self.nodes.get_mut(&n).ok_or(FsError::Gone)?;
        if f.kind == Kind::File { Ok(f) } else { Err(FsError::NotAFile) }
    }

    fn writable(&self) -> Result<(), FsError> {
        if self.read_only { Err(FsError::ReadOnly) } else { Ok(()) }
    }

    /// Changes the size of file `n` to `size`, within the capacity.
    fn resize(&mut self, n: NodeId, size: u64) -> Result<(), FsError> {
        let old = self.file_mut(n)?.data.len() as u64;
        let used = self.used - old;
        if used.checked_add(size).is_none_or(|u| u > self.capacity) {
            return Err(FsError::NoSpace);
        }
        let size = usize::try_from(size).map_err(|_| FsError::TooBig)?;
        self.file_mut(n)?.data.resize(size, 0);
        self.used = used + size as u64;
        Ok(())
    }

    /// Whether `a` is `b` or inside it.
    fn within(&self, mut a: NodeId, b: NodeId) -> bool {
        loop {
            if a == b {
                return true;
            }
            match self.nodes.get(&a) {
                Some(n) if n.parent != a => a = n.parent,
                _ => return false,
            }
        }
    }
}

impl Filesystem for MemFs {
    fn root(&self) -> NodeId {
        ROOT
    }

    fn read_only(&self) -> bool {
        self.read_only
    }

    fn lookup(&mut self, folder: NodeId, name: &str) -> Result<NodeId, FsError> {
        check_name(name)?;
        self.folder(folder)?.children.get(&key(name)).map(|c| c.1).ok_or(FsError::NotFound)
    }

    fn stat(&mut self, node: NodeId) -> Result<Stat, FsError> {
        let n = self.node(node)?;
        Ok(Stat {
            kind: n.kind,
            size: n.data.len() as u64,
            created: 0,
            modified: n.modified,
            hidden: false,
        })
    }

    fn read(&mut self, node: NodeId, offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        let f = self.file_mut(node)?;
        let len = f.data.len() as u64;
        if offset >= len {
            return Ok(0);
        }
        let n = buf.len().min((len - offset) as usize);
        buf[..n].copy_from_slice(&f.data[offset as usize..offset as usize + n]);
        Ok(n)
    }

    fn write(&mut self, node: NodeId, offset: u64, data: &[u8]) -> Result<usize, FsError> {
        self.writable()?;
        let end = offset.checked_add(data.len() as u64).ok_or(FsError::TooBig)?;
        if end > self.file_mut(node)?.data.len() as u64 {
            self.resize(node, end)?;
        }
        let f = self.file_mut(node)?;
        f.data[offset as usize..end as usize].copy_from_slice(data);
        self.dirty.insert(node, ());
        Ok(data.len())
    }

    fn set_size(&mut self, node: NodeId, size: u64) -> Result<(), FsError> {
        self.writable()?;
        self.resize(node, size)?;
        self.dirty.insert(node, ());
        Ok(())
    }

    fn commit(&mut self, node: NodeId) -> Result<(), FsError> {
        if self.dirty.remove(&node).is_some() {
            self.commits += 1;
        }
        Ok(())
    }

    fn list(
        &mut self,
        folder: NodeId,
        cursor: u64,
        max: usize,
    ) -> Result<(Vec<Entry>, u64), FsError> {
        let f = self.folder(folder)?;
        // The cursor is how many entries have been listed so far.
        let entries: Vec<Entry> = f
            .children
            .values()
            .skip(cursor as usize)
            .take(max)
            .filter_map(|(name, id)| {
                let n = self.nodes.get(id)?;
                Some(Entry {
                    name: name.clone(),
                    kind: n.kind,
                    size: n.data.len() as u64,
                    modified: n.modified,
                    hidden: false,
                })
            })
            .collect();
        let next = cursor + entries.len() as u64;
        let more = (next as usize) < f.children.len();
        Ok((entries, if more { next } else { 0 }))
    }

    fn create(&mut self, folder: NodeId, name: &str, kind: Kind) -> Result<NodeId, FsError> {
        self.writable()?;
        check_name(name)?;
        let k = key(name);
        if self.folder(folder)?.children.contains_key(&k) {
            return Err(FsError::AlreadyExists);
        }
        let id = self.next;
        self.next += 1;
        self.nodes.insert(
            id,
            Node { kind, parent: folder, data: Vec::new(), children: BTreeMap::new(), modified: 0 },
        );
        if let Some(f) = self.nodes.get_mut(&folder) {
            f.children.insert(k, (String::from(name), id));
        }
        Ok(id)
    }

    fn remove(&mut self, folder: NodeId, name: &str) -> Result<(), FsError> {
        self.writable()?;
        let id = self.lookup(folder, name)?;
        if !self.node(id)?.children.is_empty() {
            return Err(FsError::FolderNotEmpty);
        }
        if let Some(n) = self.nodes.remove(&id) {
            self.used -= n.data.len() as u64;
        }
        self.dirty.remove(&id);
        if let Some(f) = self.nodes.get_mut(&folder) {
            f.children.remove(&key(name));
        }
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
        check_name(to_name)?;
        let id = self.lookup(from, from_name)?;
        self.folder(to)?;
        if self.node(id)?.kind == Kind::Folder && self.within(to, id) {
            return Err(FsError::BadArgument);
        }
        // Another node already has the new name (the same node only changes its capitals).
        if let Ok(existing) = self.lookup(to, to_name)
            && existing != id
        {
            let folders =
                self.node(existing)?.kind == Kind::Folder || self.node(id)?.kind == Kind::Folder;
            if !replace || folders {
                return Err(FsError::AlreadyExists);
            }
            self.remove(to, to_name)?;
        }
        if let Some(f) = self.nodes.get_mut(&from) {
            f.children.remove(&key(from_name));
        }
        if let Some(t) = self.nodes.get_mut(&to) {
            t.children.insert(key(to_name), (String::from(to_name), id));
        }
        if let Some(n) = self.nodes.get_mut(&id) {
            n.parent = to;
        }
        Ok(())
    }

    fn space(&mut self) -> Result<Space, FsError> {
        Ok(Space {
            total: self.capacity,
            free: self.capacity - self.used,
            label: String::from("Memory"),
            filesystem: "MemFS",
        })
    }

    fn unmount(&mut self) -> Result<(), FsError> {
        let dirty: Vec<NodeId> = self.dirty.keys().copied().collect();
        for n in dirty {
            self.commit(n)?;
        }
        Ok(())
    }
}
