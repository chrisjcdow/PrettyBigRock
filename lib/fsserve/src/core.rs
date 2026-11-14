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
    /// The nearest limit above it (note 0026, section 4), as an index into [`Core`]'s limits,
    /// or `None` if only the server's own [`LIMIT`] applies.
    limit: Option<u32>,
}

/// A share of the records (note 0026, section 4): what is opened, created, reduced or limited
/// through a capability made by `pbr.file.limit` (and through what that opens, and so on) may
/// hold at most `max` records at once. One program with one grant could otherwise fill every
/// record the server has, and no other program could open anything on that volume.
///
/// Limits nest: a record counts against its nearest limit and every limit above that one.
/// `used` counts every record under the limit, directly or through a limit inside it, so a
/// limit is no longer needed exactly when `used` comes back to 0 (its first record was made
/// with it, so it never starts at 0); its slot is then reused.
#[derive(Clone, Copy)]
struct Share {
    max: u32,
    used: u32,
    /// The limit this one was set within.
    parent: Option<u32>,
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
    /// The limits set with `limit`; `None` for a slot free to reuse.
    shares: Vec<Option<Share>>,
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
        Core {
            fs,
            slots: vec![empty],
            free: Vec::new(),
            live: 0,
            limit,
            writers: BTreeMap::new(),
            shares: Vec::new(),
        }
    }

    /// How many records there are.
    pub fn records(&self) -> usize {
        self.live
    }

    /// How many slots the table has grown to (records are reused, so this stays small).
    pub fn slots_used(&self) -> usize {
        self.slots.len()
    }

    /// How many limits are in use (for tests: they must all go when their records do).
    pub fn limits_used(&self) -> usize {
        self.shares.iter().filter(|s| s.is_some()).count()
    }

    /// A record for the root folder, with every right: the badge the server mints itself.
    pub fn root(&mut self) -> Result<Badge, Error> {
        let root = self.fs.root();
        self.add(root, Kind::Folder, ALL, None)
    }

    /// The limits from `limit` upwards, nearest first.
    fn chain(&self, mut limit: Option<u32>) -> Vec<u32> {
        let mut out = Vec::new();
        while let Some(i) = limit {
            out.push(i);
            limit = self.shares[i as usize].and_then(|s| s.parent);
        }
        out
    }

    /// Whether one more record fits under `limit`, every limit above it, and the server.
    fn room(&self, limit: Option<u32>) -> bool {
        self.live < self.limit
            && self
                .chain(limit)
                .iter()
                .all(|&i| self.shares[i as usize].is_none_or(|s| s.used < s.max))
    }

    /// Adds a record under `limit` (the nearest limit above it), if that and every limit above
    /// it has room, and the server as a whole has.
    fn add(
        &mut self,
        node: NodeId,
        kind: Kind,
        rights: u32,
        limit: Option<u32>,
    ) -> Result<Badge, Error> {
        if !self.room(limit) {
            return Err(Error::TooManyOpen);
        }
        for i in self.chain(limit) {
            if let Some(s) = &mut self.shares[i as usize] {
                s.used += 1;
            }
        }
        let index = match self.free.pop() {
            Some(i) => i,
            None => {
                self.slots.push(Slot { generation: 0, object: None });
                (self.slots.len() - 1) as u32
            }
        };
        let slot = &mut self.slots[index as usize];
        slot.object = Some(Object { node, kind, rights, wrote: false, limit });
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
        // It no longer counts against its limits; a limit with nothing left under it goes.
        for i in self.chain(o.limit) {
            let share = &mut self.shares[i as usize];
            if let Some(s) = share {
                s.used -= 1;
                if s.used == 0 {
                    *share = None;
                }
            }
        }
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
        self.add(o.node, o.kind, rights, o.limit)
    }

    /// Like [`Core::reduce`], but the new record and everything made through it may hold at
    /// most `records` records at once, counting the new record itself; those also count against
    /// every limit above it (note 0026, section 4). A limit can be set within a limit, never
    /// above it: `records` may not be more than the nearest limit's own.
    pub fn limit(&mut self, badge: Badge, rights: u32, records: u32) -> Result<Badge, Error> {
        let o = self.get(badge)?;
        if rights & !ALL != 0 || records == 0 {
            return Err(Error::BadArgument);
        }
        if let Some(i) = o.limit
            && self.shares[i as usize].is_some_and(|s| records > s.max)
        {
            return Err(Error::BadArgument);
        }
        Self::need(&o, rights)?;
        self.fs.stat(o.node)?;
        // The new limit takes a free slot (or a new one); it starts with nothing under it and
        // gets its first record from `add`, which also checks the limits above have room.
        let share = Share { max: records, used: 0, parent: o.limit };
        let at = match self.shares.iter().position(|s| s.is_none()) {
            Some(i) => {
                self.shares[i] = Some(share);
                i
            }
            None => {
                self.shares.push(Some(share));
                self.shares.len() - 1
            }
        } as u32;
        match self.add(o.node, o.kind, rights, Some(at)) {
            Ok(b) => Ok(b),
            Err(e) => {
                self.shares[at as usize] = None;
                Err(e)
            }
        }
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

    /// Makes the file's changes durable. Needs `WRITE`: otherwise a program that can only read
    /// a file could make another's half-finished save durable, and a crash then would leave the
    /// file half saved, which "write everything, then `sync`" promises never happens on RockFS
    /// (note 0014, section 6).
    pub fn sync(&mut self, badge: Badge) -> Result<(), Error> {
        let o = self.file(badge)?;
        Self::need(&o, WRITE)?;
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
        let b = self.add(node, stat.kind, rights, o.limit)?;
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
        // Room for its record is checked first, so that a full share doesn't leave behind a
        // file made but not opened.
        if !self.room(o.limit) {
            return Err(Error::TooManyOpen);
        }
        let node = self.fs.create(parent, last, kind)?;
        let stat = self.fs.stat(node)?;
        let b = self.add(node, kind, rights, o.limit)?;
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
