// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The rules every filesystem server enforces (docs/design/0014-files-and-file-trees.md,
//! sections 2 to 4 and 7), as plain logic that the host tests drive directly.
//!
//! A program refers to a file or folder through a capability: an endpoint capability to this
//! server carrying a **badge**, a number the kernel attaches to every request made through it.
//! The badge picks a record in the **object table**: which node (file or folder) and which
//! rights. Everything here starts from "which badge is asking?", so a program can never name a
//! record it wasn't given: it can only send requests through capabilities it holds.
//!
//! The server never climbs out of a folder: paths only go down (no `..`), so a folder
//! capability confines its holder to that folder and what's inside it.

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;
use pbr_fs::{Entry, Filesystem, FsError, Kind, NodeId, Space, Stat};

/// The right to read: a file's contents, a folder's listing.
pub const READ: u32 = 1;
/// The right to change: a file's contents, a folder's entries.
pub const WRITE: u32 = 2;
/// Every right there is.
pub const ALL: u32 = READ | WRITE;

/// The most records a server keeps (note 0014, section 3).
pub const LIMIT: usize = 65536;
/// The longest path, in bytes.
pub const MAX_PATH: usize = 4096;
/// The longest name, in characters.
pub const MAX_NAME: usize = 255;
/// The most bytes one read or write moves.
pub const MAX_IO: usize = 65536;
/// The most entries in one listing (`list<Entry, 56>` in `pbr.file`).
pub const MAX_LIST: usize = 56;

/// A badge: a record's slot in the low 32 bits, the slot's generation in bits 32 to 62. Never
/// 0 (slot 0 is never used), and bit 63 is never set, so badges with it set (like the control
/// badge) can't collide with records.
pub type Badge = u64;

/// What can go wrong, as `pbr.file` says it (note 0014, section 5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// No such name.
    NotFound,
    /// The name is taken.
    AlreadyExists,
    /// A folder was needed.
    NotAFolder,
    /// A file was needed.
    NotAFile,
    /// The capability lacks the right.
    NoRight,
    /// A name breaks the rules.
    BadName,
    /// Empty names, `.`, `..`, a leading `/`, or too long.
    BadPath,
    /// Only an empty folder can be removed.
    FolderNotEmpty,
    /// Not enough free space.
    NoSpace,
    /// The volume can't be written.
    ReadOnly,
    /// Beyond what the filesystem holds.
    TooBig,
    /// The node was removed.
    Gone,
    /// The volume's structures are damaged here.
    Damaged,
    /// The disk failed.
    DiskError,
    /// The record limit has been reached.
    TooManyOpen,
    /// Anything else wrong with the request.
    BadArgument,
}

impl From<FsError> for Error {
    fn from(e: FsError) -> Error {
        match e {
            FsError::NotFound => Error::NotFound,
            FsError::AlreadyExists => Error::AlreadyExists,
            FsError::NotAFolder => Error::NotAFolder,
            FsError::NotAFile => Error::NotAFile,
            FsError::BadName => Error::BadName,
            FsError::FolderNotEmpty => Error::FolderNotEmpty,
            FsError::NoSpace => Error::NoSpace,
            FsError::ReadOnly => Error::ReadOnly,
            FsError::TooBig => Error::TooBig,
            FsError::Gone => Error::Gone,
            FsError::Damaged => Error::Damaged,
            FsError::Disk => Error::DiskError,
            FsError::BadArgument => Error::BadArgument,
        }
    }
}

/// Facts about a node, as a capability for it sees them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Info {
    /// The node's facts.
    pub stat: Stat,
    /// The capability's rights.
    pub rights: u32,
    /// The volume can't be written.
    pub read_only: bool,
}

/// One record: what a badge stands for.
#[derive(Clone, Copy)]
struct Object {
    node: NodeId,
    kind: Kind,
    rights: u32,
    /// Changes were made through it, so its going may complete a save (note 0014, section 6).
    wrote: bool,
}

struct Slot {
    generation: u32,
    object: Option<Object>,
}

/// The server's records, and the filesystem behind them.
pub struct Core<F: Filesystem> {
    /// The filesystem.
    pub fs: F,
    slots: Vec<Slot>,
    free: Vec<u32>,
    live: usize,
    limit: usize,
    /// For each node with changes not yet committed, how many records made changes to it.
    writers: BTreeMap<NodeId, usize>,
}

/// Checks a path against note 0014's rules and splits it into names.
pub fn split_path(path: &str) -> Result<Vec<&str>, Error> {
    if path.is_empty() || path.len() > MAX_PATH {
        return Err(Error::BadPath);
    }
    let mut names = Vec::new();
    for name in path.split('/') {
        if name.is_empty() || name == "." || name == ".." {
            return Err(Error::BadPath); // a leading or doubled `/`, or a step sideways or up
        }
        check_name(name)?;
        names.push(name);
    }
    Ok(names)
}

/// The rules for every name, whatever the filesystem (note 0014, section 4).
pub fn check_name(name: &str) -> Result<(), Error> {
    let chars = name.chars().count();
    if chars == 0 || chars > MAX_NAME || name == "." || name == ".." {
        return Err(Error::BadName);
    }
    if name.chars().any(|c| c == '/' || (c as u32) < 32 || c as u32 == 127) {
        return Err(Error::BadName);
    }
    Ok(())
}

impl<F: Filesystem> Core<F> {
    /// A server for `fs`, with no records yet.
    pub fn new(fs: F) -> Core<F> {
        Core::with_limit(fs, LIMIT)
    }

    /// The same, with a smaller limit on records (for tests).
    pub fn with_limit(fs: F, limit: usize) -> Core<F> {
        let empty = Slot { generation: 0, object: None };
        Core { fs, slots: vec![empty], free: Vec::new(), live: 0, limit, writers: BTreeMap::new() }
    }

    /// How many records there are.
    pub fn records(&self) -> usize {
        self.live
    }

    /// How many slots the table has grown to (records are reused, so this stays small).
    pub fn slots_used(&self) -> usize {
        self.slots.len()
    }

    /// A record for the root folder, with every right: the badge the server mints itself.
    pub fn root(&mut self) -> Result<Badge, Error> {
        let root = self.fs.root();
        self.add(root, Kind::Folder, ALL)
    }

    fn add(&mut self, node: NodeId, kind: Kind, rights: u32) -> Result<Badge, Error> {
        if self.live >= self.limit {
            return Err(Error::TooManyOpen);
        }
        let index = match self.free.pop() {
            Some(i) => i,
            None => {
                self.slots.push(Slot { generation: 0, object: None });
                (self.slots.len() - 1) as u32
            }
        };
        let slot = &mut self.slots[index as usize];
        slot.object = Some(Object { node, kind, rights, wrote: false });
        self.live += 1;
        Ok(u64::from(slot.generation) << 32 | u64::from(index))
    }

    /// The record `badge` names. The kernel only delivers badges this server minted, so an
    /// unknown one means its record went with a gone notice: the capability no longer exists.
    fn get(&self, badge: Badge) -> Result<Object, Error> {
        let (index, generation) = ((badge & 0xFFFF_FFFF) as usize, (badge >> 32) as u32);
        match self.slots.get(index) {
            Some(Slot { generation: g, object: Some(o) }) if *g == generation && index != 0 => {
                Ok(*o)
            }
            _ => Err(Error::BadArgument),
        }
    }

    /// Every capability carrying `badge` has gone: forget the record, and if it was the last
    /// one through which its file was changed, make the changes durable (note 0014, section 6).
    /// Also called when a reply carrying a new capability couldn't be delivered.
    pub fn gone(&mut self, badge: Badge) {
        let Ok(o) = self.get(badge) else { return };
        let index = (badge & 0xFFFF_FFFF) as usize;
        let slot = &mut self.slots[index];
        slot.object = None;
        // Generations wrap within 31 bits, so bit 63 of a badge is never set.
        slot.generation = (slot.generation + 1) & 0x7FFF_FFFF;
        self.free.push(index as u32);
        self.live -= 1;
        if o.wrote {
            let left = match self.writers.get_mut(&o.node) {
                Some(n) => {
                    *n -= 1;
                    *n
                }
                None => 0,
            };
            if left == 0 {
                self.writers.remove(&o.node);
                // A file removed meanwhile is `Gone`: there's nothing left to save.
                let _ = self.fs.commit(o.node);
            }
        }
    }

    /// Needs `right` on `o`.
    fn need(o: &Object, right: u32) -> Result<(), Error> {
        if o.rights & right == right { Ok(()) } else { Err(Error::NoRight) }
    }

    fn folder(&self, badge: Badge) -> Result<Object, Error> {
        let o = self.get(badge)?;
        if o.kind != Kind::Folder {
            return Err(Error::NotAFolder);
        }
        Ok(o)
    }

    fn file(&self, badge: Badge) -> Result<Object, Error> {
        let o = self.get(badge)?;
        if o.kind != Kind::File {
            return Err(Error::NotAFile);
        }
        Ok(o)
    }

    /// Follows `names` down from `node`. Every step must be a folder.
    fn walk(&mut self, mut node: NodeId, names: &[&str]) -> Result<NodeId, Error> {
        for name in names {
            if self.fs.stat(node)?.kind != Kind::Folder {
                return Err(Error::NotAFolder);
            }
            node = self.fs.lookup(node, name)?;
        }
        Ok(node)
    }

    /// Facts about what `badge` names.
    pub fn info(&mut self, badge: Badge) -> Result<Info, Error> {
        let o = self.get(badge)?;
        let stat = self.fs.stat(o.node)?;
        Ok(Info { stat, rights: o.rights, read_only: self.fs.read_only() })
    }

    /// A new record for the same node with only `rights` (which must be a subset).
    pub fn reduce(&mut self, badge: Badge, rights: u32) -> Result<Badge, Error> {
        let o = self.get(badge)?;
        if rights & !ALL != 0 {
            return Err(Error::BadArgument);
        }
        Self::need(&o, rights)?;
        self.fs.stat(o.node)?; // a removed node stays removed
        self.add(o.node, o.kind, rights)
    }

    /// The volume's size and free space.
    pub fn space(&mut self, badge: Badge) -> Result<Space, Error> {
        self.get(badge)?;
        Ok(self.fs.space()?)
    }

    /// Reads up to `max` bytes from `offset`.
    pub fn read(&mut self, badge: Badge, offset: u64, max: u32) -> Result<Vec<u8>, Error> {
        let o = self.file(badge)?;
        Self::need(&o, READ)?;
        let max = (max as usize).min(MAX_IO);
        let mut buf = vec![0u8; max];
        let n = self.fs.read(o.node, offset, &mut buf)?;
        buf.truncate(n.min(max));
        Ok(buf)
    }

    /// Notes that the record at `badge` changed its file.
    fn wrote(&mut self, badge: Badge, o: Object) {
        if !o.wrote {
            if let Some(Slot { object: Some(obj), .. }) =
                self.slots.get_mut((badge & 0xFFFF_FFFF) as usize)
            {
                obj.wrote = true;
            }
            *self.writers.entry(o.node).or_insert(0) += 1;
        }
    }

    /// Writes `data` at `offset`.
    pub fn write(&mut self, badge: Badge, offset: u64, data: &[u8]) -> Result<u32, Error> {
        let o = self.file(badge)?;
        Self::need(&o, WRITE)?;
        if data.len() > MAX_IO {
            return Err(Error::BadArgument);
        }
        let n = self.fs.write(o.node, offset, data)?;
        self.wrote(badge, o);
        Ok(n as u32)
    }

    /// Sets the file's size.
    pub fn set_size(&mut self, badge: Badge, size: u64) -> Result<(), Error> {
        let o = self.file(badge)?;
        Self::need(&o, WRITE)?;
        self.fs.set_size(o.node, size)?;
        self.wrote(badge, o);
        Ok(())
    }

    /// Makes the file's changes durable.
    pub fn sync(&mut self, badge: Badge) -> Result<(), Error> {
        let o = self.file(badge)?;
        Ok(self.fs.commit(o.node)?)
    }

    /// Opens `path` below the folder `badge` names, with `rights` (at most the folder's).
    pub fn open(&mut self, badge: Badge, path: &str, rights: u32) -> Result<(Badge, Info), Error> {
        let o = self.folder(badge)?;
        if rights & !ALL != 0 {
            return Err(Error::BadArgument);
        }
        Self::need(&o, rights)?;
        let names = split_path(path)?;
        let node = self.walk(o.node, &names)?;
        let stat = self.fs.stat(node)?;
        let b = self.add(node, stat.kind, rights)?;
        Ok((b, Info { stat, rights, read_only: self.fs.read_only() }))
    }

    /// Creates `path`'s last name below the folder `badge` names (which needs `WRITE`), and
    /// opens it with `rights` (at most the folder's).
    pub fn create(
        &mut self,
        badge: Badge,
        path: &str,
        kind: Kind,
        rights: u32,
    ) -> Result<(Badge, Info), Error> {
        let o = self.folder(badge)?;
        if rights & !ALL != 0 {
            return Err(Error::BadArgument);
        }
        Self::need(&o, WRITE | rights)?;
        let names = split_path(path)?;
        let (last, parents) = names.split_last().ok_or(Error::BadPath)?;
        let parent = self.walk(o.node, parents)?;
        if self.fs.stat(parent)?.kind != Kind::Folder {
            return Err(Error::NotAFolder);
        }
        let node = self.fs.create(parent, last, kind)?;
        let stat = self.fs.stat(node)?;
        let b = self.add(node, kind, rights)?;
        Ok((b, Info { stat, rights, read_only: self.fs.read_only() }))
    }

    /// Lists the folder from `cursor`: at most `max` entries, and never more than
    /// [`MAX_LIST`], the most `pbr.file`'s reply can carry with the longest names.
    pub fn list(
        &mut self,
        badge: Badge,
        cursor: u64,
        max: u32,
    ) -> Result<(Vec<Entry>, u64), Error> {
        let o = self.folder(badge)?;
        Self::need(&o, READ)?;
        let max = (max as usize).min(MAX_LIST);
        let mut out: Vec<Entry> = Vec::new();
        let next = self.fs.list(o.node, cursor, &mut |e| {
            if out.len() >= max {
                return false;
            }
            out.push(e);
            true
        })?;
        Ok((out, next))
    }

    /// Removes the file or empty folder `path` names, below the folder `badge` names.
    pub fn remove(&mut self, badge: Badge, path: &str) -> Result<(), Error> {
        let o = self.folder(badge)?;
        Self::need(&o, WRITE)?;
        let names = split_path(path)?;
        let (last, parents) = names.split_last().ok_or(Error::BadPath)?;
        let parent = self.walk(o.node, parents)?;
        Ok(self.fs.remove(parent, last)?)
    }

    /// Renames or moves `from` to `to`, both below the folder `badge` names.
    pub fn rename(
        &mut self,
        badge: Badge,
        from: &str,
        to: &str,
        replace: bool,
    ) -> Result<(), Error> {
        let o = self.folder(badge)?;
        Self::need(&o, WRITE)?;
        let (f, t) = (split_path(from)?, split_path(to)?);
        let (f_last, f_parents) = f.split_last().ok_or(Error::BadPath)?;
        let (t_last, t_parents) = t.split_last().ok_or(Error::BadPath)?;
        let from_parent = self.walk(o.node, f_parents)?;
        let to_parent = self.walk(o.node, t_parents)?;
        Ok(self.fs.rename(from_parent, f_last, to_parent, t_last, replace)?)
    }

    /// Commits everything and marks the volume clean.
    pub fn unmount(&mut self) -> Result<(), Error> {
        self.writers.clear();
        Ok(self.fs.unmount()?)
    }
}
