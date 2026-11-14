// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Files for programs (docs/design/0014-files-and-file-trees.md, section 8): a program's file
//! tree, and opening, listing and changing files by path.
//!
//! A program starts with a file tree: entries such as `/Home/Documents`, each with a capability
//! for the folder or file there (a capability is the right to use something, held by the
//! kernel on the program's behalf; note 0006). [`Files`] turns a path into a request to the
//! right capability, so a program can say `files.open("/Home/Documents/letter.txt", READ)`
//! like on any other system. Everything outside its entries simply isn't there.
//!
//! (The design notes call this `granite::fs`; it is its own library because it uses the code
//! generated from `pbr.file`, which itself is built on `granite`.)

#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use granite::endpoint::Endpoint;
use granite::interface::CallError;
use granite::memory::Memory;
use granite::process::role;
use granite::{Handle, sys};
use pbr_filetree::{Found, Kind, Mount, Tree, normalize};
/// The errors a filesystem server gives, re-exported so programs can match on them.
pub use pbr_interfaces::pbr_file::FileError;
use pbr_interfaces::pbr_file::{self as file, NodeInfo};

pub use pbr_interfaces::pbr_file::{FileRights, NodeKind};

/// The right to read.
pub const READ: FileRights = FileRights::READ;
/// The right to change.
pub const WRITE: FileRights = FileRights::WRITE;
/// Both.
pub const READ_WRITE: FileRights = FileRights(FileRights::READ.0 | FileRights::WRITE.0);

/// What can go wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Nothing this program holds is at that path.
    NotFound,
    /// The path breaks the rules.
    BadPath,
    /// A folder above the program's entries: it only lists what the program holds, and can't
    /// be changed.
    Above,
    /// The filesystem said no.
    File(FileError),
    /// The request couldn't be made (the server has gone, a capability was revoked).
    Call(granite::Error),
    /// The answer was malformed.
    Malformed,
}

impl From<CallError<FileError>> for Error {
    fn from(e: CallError<FileError>) -> Error {
        match e {
            CallError::Service(e) => Error::File(e),
            CallError::Call(e) => Error::Call(e),
            _ => Error::Malformed,
        }
    }
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::NotFound => write!(f, "not found"),
            Error::BadPath => write!(f, "not a valid path"),
            Error::Above => write!(f, "that folder can't be changed"),
            Error::File(e) => write!(f, "{e:?}"),
            Error::Call(e) => write!(f, "{e}"),
            Error::Malformed => write!(f, "a malformed answer"),
        }
    }
}

/// `Result` with this library's error.
pub type Result<T> = core::result::Result<T, Error>;

/// One thing in a folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listing {
    /// Its name.
    pub name: String,
    /// File or folder.
    pub folder: bool,
    /// Bytes, for a file.
    pub size: u64,
    /// When it last changed (seconds since 1970 UTC), 0 if unknown.
    pub modified: u64,
    /// Hidden from ordinary listings (shown only when asked).
    pub hidden: bool,
}

/// An open file or folder: its own capability, closed when this is dropped.
#[derive(Debug)]
pub struct File {
    client: file::Client,
}

impl File {
    fn new(ep: Endpoint) -> File {
        File { client: file::Client::new(ep) }
    }

    /// Facts about it.
    pub fn info(&self) -> Result<NodeInfo> {
        Ok(self.client.info()?)
    }

    /// Up to `max` bytes (at most 65536) from `offset`; fewer only at the end.
    pub fn read_at(&self, offset: u64, max: u32) -> Result<Vec<u8>> {
        Ok(self.client.read(offset, max)?)
    }

    /// The whole file.
    pub fn read_all(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        loop {
            let chunk = self.read_at(out.len() as u64, 65536)?;
            if chunk.is_empty() {
                return Ok(out);
            }
            out.extend_from_slice(&chunk);
        }
    }

    /// Writes `data` at `offset`.
    pub fn write_at(&self, offset: u64, data: &[u8]) -> Result<()> {
        for (i, chunk) in data.chunks(65536).enumerate() {
            self.client.write(offset + (i * 65536) as u64, chunk)?;
        }
        Ok(())
    }

    /// Replaces the whole contents with `data`, and makes it durable: on RockFS, an atomic
    /// save (note 0014, section 6).
    pub fn save(&self, data: &[u8]) -> Result<()> {
        self.client.set_size(0)?;
        self.write_at(0, data)?;
        Ok(self.client.sync()?)
    }

    /// Sets the size.
    pub fn set_size(&self, size: u64) -> Result<()> {
        Ok(self.client.set_size(size)?)
    }

    /// Makes the changes durable.
    pub fn sync(&self) -> Result<()> {
        Ok(self.client.sync()?)
    }

    /// A copy with fewer rights, revoked with this one.
    pub fn reduce(&self, rights: FileRights) -> Result<File> {
        Ok(File { client: self.client.reduce(rights)? })
    }

    /// A copy with fewer rights, revoked with this one, under which at most `records` records
    /// may be open at once (note 0026, section 4).
    pub fn limit(&self, rights: FileRights, records: u32) -> Result<File> {
        Ok(File { client: self.client.limit(rights, records)? })
    }

    /// Everything in this folder.
    pub fn list(&self) -> Result<Vec<Listing>> {
        let mut out = Vec::new();
        let mut cursor = 0;
        loop {
            let r = self.client.list(cursor, 56)?;
            out.extend(r.entries.into_iter().map(|e| Listing {
                name: e.name,
                folder: e.kind == NodeKind::Folder,
                size: e.size,
                modified: e.modified,
                hidden: e.hidden,
            }));
            if r.next == 0 {
                return Ok(out);
            }
            cursor = r.next;
        }
    }

    /// The capability, for passing on.
    pub fn handle(&self) -> &Handle {
        &self.client.endpoint().0
    }

    /// The capability itself, for keeping: what a launcher holds so it can revoke what it gave.
    pub fn into_handle(self) -> Handle {
        self.client.into_endpoint().0
    }
}

/// A program's file tree, and its current folder.
pub struct Files {
    tree: Tree,
    caps: Vec<File>,
    current: String,
}

impl Files {
    /// An empty tree: a program given no files.
    pub fn empty() -> Files {
        Files { tree: Tree::default(), caps: Vec::new(), current: String::from("/") }
    }

    /// The tree this program was started with (note 0014, section 8). A missing tree is
    /// empty; a broken one is reported and treated as empty.
    pub fn from_start(start: &mut granite::Start) -> Files {
        let Some(h) = start.take(role::FILE_TREE) else { return Files::empty() };
        let mem = Memory(h);
        let size = mem.size().unwrap_or(0).min(pbr_filetree::MAX_BYTES as u64) as usize;
        let mut bytes = vec![0u8; size];
        let mounts =
            match mem.read(0, &mut bytes).ok().and_then(|()| pbr_filetree::decode(&bytes).ok()) {
                Some(m) => m,
                None => {
                    granite::println!(
                        "files: this program's file tree is damaged; it has no files."
                    );
                    return Files::empty();
                }
            };
        let mut caps = Vec::new();
        for i in 0..mounts.len() {
            match start.take(role::FILE_TREE_FIRST + i as u32) {
                Some(h) => caps.push(File::new(Endpoint(h))),
                None => {
                    granite::println!(
                        "files: entry {i} of the file tree came without its capability."
                    );
                    return Files::empty();
                }
            }
        }
        let tree = Tree::new(mounts).unwrap_or_default();
        Files { tree, caps, current: String::from("/") }
    }

    /// Where app `app`'s own folder is in its tree (docs/design/0024-the-session-desktop.md,
    /// section 4): `/Home/Apps/APP`, the same path the user sees it at. The sandbox manager
    /// makes it the first time an app whose manifest says `data` is launched.
    pub fn data_folder(app: &str) -> String {
        alloc::format!("/Home/Apps/{app}")
    }

    /// The entries: (path, folder).
    pub fn entries(&self) -> Vec<(String, bool)> {
        self.tree.mounts().iter().map(|m| (m.path.clone(), m.kind == Kind::Folder)).collect()
    }

    /// Adds `cap`, a capability for a file or folder, to the tree at `path`.
    pub fn mount(&mut self, path: &str, cap: Handle) -> Result<()> {
        let path = self.absolute(path)?;
        let f = File::new(Endpoint(cap));
        let folder = f.info()?.kind == NodeKind::Folder;
        let kind = if folder { Kind::Folder } else { Kind::File };
        self.tree.add(Mount { path, kind }).map_err(|_| Error::BadPath)?;
        self.caps.push(f);
        Ok(())
    }

    /// The current folder.
    pub fn current(&self) -> &str {
        &self.current
    }

    /// Changes the current folder.
    pub fn set_current(&mut self, path: &str) -> Result<()> {
        let p = self.absolute(path)?;
        match self.tree.find(&p) {
            Found::Above(_) => {}
            Found::Nothing => return Err(Error::NotFound),
            Found::Entry { .. } => {
                if self.info(&p)?.kind != NodeKind::Folder {
                    return Err(Error::File(FileError::NotAFolder));
                }
            }
        }
        self.current = p;
        Ok(())
    }

    /// `path` made absolute and tidy, from the current folder.
    pub fn absolute(&self, path: &str) -> Result<String> {
        normalize(&self.current, path).map_err(|_| Error::BadPath)
    }

    /// Opens `path` with `rights`.
    pub fn open(&self, path: &str, rights: FileRights) -> Result<File> {
        let p = self.absolute(path)?;
        match self.tree.find(&p) {
            // The entry itself: a reduced copy of its capability, so the tree keeps its own.
            Found::Entry { index, rest: "" } => self.caps[index].reduce(rights),
            Found::Entry { index, rest } => {
                let r = self.caps[index].client.open(rest, rights)?;
                Ok(File { client: r.node })
            }
            Found::Above(_) => Err(Error::Above),
            Found::Nothing => Err(Error::NotFound),
        }
    }

    /// Creates a file or folder at `path`, and opens it with `rights`.
    pub fn create(&self, path: &str, folder: bool, rights: FileRights) -> Result<File> {
        let p = self.absolute(path)?;
        let kind = if folder { NodeKind::Folder } else { NodeKind::File };
        match self.tree.find(&p) {
            Found::Entry { index, rest } if !rest.is_empty() => {
                let r = self.caps[index].client.create(rest, kind, rights)?;
                Ok(File { client: r.node })
            }
            Found::Entry { .. } => Err(Error::File(FileError::AlreadyExists)),
            Found::Above(_) | Found::Nothing => Err(Error::Above),
        }
    }

    /// Facts about `path`. A folder above the entries reports itself as a read-only folder.
    pub fn info(&self, path: &str) -> Result<NodeInfo> {
        let p = self.absolute(path)?;
        match self.tree.find(&p) {
            Found::Entry { index, rest: "" } => self.caps[index].info(),
            Found::Entry { index, rest } => {
                let r = self.caps[index].client.open(rest, FileRights(0))?;
                Ok(r.info)
            }
            Found::Above(_) => Ok(NodeInfo {
                kind: NodeKind::Folder,
                rights: READ,
                size: 0,
                created: 0,
                modified: 0,
                hidden: false,
                read_only: true,
            }),
            Found::Nothing => Err(Error::NotFound),
        }
    }

    /// What's in the folder at `path`.
    pub fn list(&self, path: &str) -> Result<Vec<Listing>> {
        let p = self.absolute(path)?;
        match self.tree.find(&p) {
            Found::Above(children) => Ok(children
                .into_iter()
                .map(|(name, kind)| Listing {
                    name,
                    folder: kind == Kind::Folder,
                    size: 0,
                    modified: 0,
                    hidden: false,
                })
                .collect()),
            Found::Nothing => Err(Error::NotFound),
            Found::Entry { .. } => self.open(&p, READ)?.list(),
        }
    }

    /// Removes the file or empty folder at `path` (not an entry itself).
    pub fn remove(&self, path: &str) -> Result<()> {
        let p = self.absolute(path)?;
        match self.tree.find(&p) {
            Found::Entry { index, rest } if !rest.is_empty() => {
                Ok(self.caps[index].client.remove(rest)?)
            }
            Found::Nothing => Err(Error::NotFound),
            _ => Err(Error::Above),
        }
    }

    /// Renames or moves `from` to `to`. Within one entry the filesystem does it in one step;
    /// between entries a file is copied and then removed (a folder can't be).
    pub fn rename(&self, from: &str, to: &str) -> Result<()> {
        let (f, t) = (self.absolute(from)?, self.absolute(to)?);
        match (self.tree.find(&f), self.tree.find(&t)) {
            (Found::Entry { index: a, rest: fr }, Found::Entry { index: b, rest: tr })
                if a == b && !fr.is_empty() && !tr.is_empty() =>
            {
                Ok(self.caps[a].client.rename(fr, tr, false)?)
            }
            (Found::Entry { rest: fr, .. }, Found::Entry { rest: tr, .. })
                if !fr.is_empty() && !tr.is_empty() =>
            {
                let src = self.open(&f, READ)?;
                if src.info()?.kind != NodeKind::File {
                    return Err(Error::File(FileError::BadArgument));
                }
                let data = src.read_all()?;
                let dst = self.create(&t, false, READ_WRITE)?;
                dst.save(&data)?;
                self.remove(&f)
            }
            (Found::Nothing, _) => Err(Error::NotFound),
            _ => Err(Error::Above),
        }
    }
}

/// The type of a raw handle, for tests that look at what a program holds: (object type,
/// rights), or `None` if the number names nothing.
pub fn handle_kind(h: u32) -> Option<(u32, u32)> {
    sys::call(sys::num::HANDLE_INFO, [u64::from(h), 0, 0, 0, 0, 0])
        .ok()
        .map(|(t, r)| (t as u32, r as u32))
}

/// Text for an error, for messages.
pub fn describe(e: Error) -> String {
    format!("{e}")
}
