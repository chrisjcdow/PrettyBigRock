// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! A file tree, as browsing needs it: the [`Folders`] trait.
//!
//! A *trait* is Rust's way of saying what a type must be able to do (here: list a folder, say
//! what a path is, make a folder) without saying how. The picker and Files are written against
//! the trait, so the same code browses a program's real file tree on PrettyBigRock and
//! [`MemTree`], a tree in memory, in the host tests.

use crate::item::Item;
use crate::path;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// What can go wrong while browsing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrowseError {
    /// Nothing at that path.
    NotFound,
    /// A folder was needed.
    NotAFolder,
    /// The name is taken.
    AlreadyExists,
    /// The program may not do that here (a folder it may only read).
    NoRight,
    /// A name or path that breaks the rules ([`crate::path::check_name`]).
    BadName,
    /// Anything else, as the filesystem said it.
    Other(String),
}

/// A file tree as browsing sees it. Paths are absolute and tidy (`/Home/Documents`, never
/// `/Home/./x` or a trailing `/`); `/` is the top of the program's own tree.
pub trait Folders {
    /// Everything in the folder at `path`, in no particular order.
    fn list(&mut self, path: &str) -> Result<Vec<Item>, BrowseError>;
    /// What is at `path` (its `name` is the last part of the path; `/` is named `/`).
    fn stat(&mut self, path: &str) -> Result<Item, BrowseError>;
    /// Makes a new, empty folder at `path` (its parent must be there).
    fn make_folder(&mut self, path: &str) -> Result<(), BrowseError>;
}

/// What is at one path of a [`MemTree`].
#[derive(Clone, Debug)]
struct Node {
    folder: bool,
    size: u64,
    modified: u64,
    hidden: bool,
    /// Whether `make_folder` may not make anything below this folder.
    read_only: bool,
}

impl Node {
    fn folder() -> Node {
        Node { folder: true, size: 0, modified: 0, hidden: false, read_only: false }
    }
}

/// A tree in memory, for tests (and for anything on the host that wants to pretend).
///
/// It keeps every folder and file in one `BTreeMap` (a sorted map: look something up by its
/// key, here its whole path), which is plenty for a test and easy to follow. The top, `/`, is
/// always there.
#[derive(Clone, Debug)]
pub struct MemTree {
    nodes: BTreeMap<String, Node>,
}

impl Default for MemTree {
    fn default() -> MemTree {
        MemTree::new()
    }
}

impl MemTree {
    /// A tree with just `/`.
    pub fn new() -> MemTree {
        let mut nodes = BTreeMap::new();
        nodes.insert(String::from("/"), Node::folder());
        MemTree { nodes }
    }

    /// Makes the folder at `path` and any folders above it that are missing. A path that is not
    /// tidy, has a name that [`path::check_name`] refuses, or runs through a file is ignored.
    /// (A method that returns `&mut MemTree` lets calls be chained: `tree.add_folder(..).hide(..)`.)
    pub fn add_folder(&mut self, path: &str) -> &mut MemTree {
        self.make_folders(path);
        self
    }

    /// Makes a file (and the folders above it that are missing), replacing a file already
    /// there. Bad paths, and a path that is a folder already, are ignored as for `add_folder`.
    pub fn add_file(&mut self, path: &str, size: u64, modified: u64) -> &mut MemTree {
        let Some(parent) = path::parent(path) else {
            return self;
        };
        if path::tidy(path).is_none()
            || path::check_name(path::name(path)).is_err()
            || self.nodes.get(path).is_some_and(|n| n.folder)
        {
            return self;
        }
        if self.make_folders(parent) {
            self.nodes.insert(
                String::from(path),
                Node { folder: false, size, modified, hidden: false, read_only: false },
            );
        }
        self
    }

    /// Marks what is at `path` as hidden (nothing happens if it is not there).
    pub fn hide(&mut self, path: &str) -> &mut MemTree {
        if path != "/"
            && let Some(node) = self.nodes.get_mut(path)
        {
            node.hidden = true;
        }
        self
    }

    /// Makes the folder at `path` read-only: `make_folder` anywhere below it answers
    /// [`BrowseError::NoRight`]. (Nothing happens if it is not a folder.)
    pub fn read_only(&mut self, path: &str) -> &mut MemTree {
        if let Some(node) = self.nodes.get_mut(path).filter(|n| n.folder) {
            node.read_only = true;
        }
        self
    }

    /// Whether anything is at `path`.
    pub fn exists(&self, path: &str) -> bool {
        self.nodes.contains_key(path)
    }

    /// Makes `path` and its missing parents as folders; true if it is a folder afterwards.
    fn make_folders(&mut self, path: &str) -> bool {
        if path::tidy(path).is_none() {
            return false;
        }
        // Check every name first, so a bad one halfway down makes nothing at all.
        let crumbs = path::crumbs(path);
        if crumbs.iter().skip(1).any(|(name, _)| path::check_name(name).is_err()) {
            return false;
        }
        for (_, so_far) in &crumbs {
            match self.nodes.get(so_far.as_str()) {
                Some(node) if node.folder => {}
                Some(_) => return false,
                None => {
                    self.nodes.insert(so_far.clone(), Node::folder());
                }
            }
        }
        true
    }

    /// The item for a path and its node.
    fn item(path: &str, node: &Node) -> Item {
        Item {
            name: String::from(path::name(path)),
            folder: node.folder,
            size: node.size,
            modified: node.modified,
            hidden: node.hidden,
        }
    }
}

impl Folders for MemTree {
    fn list(&mut self, path: &str) -> Result<Vec<Item>, BrowseError> {
        path::tidy(path).ok_or(BrowseError::BadName)?;
        let node = self.nodes.get(path).ok_or(BrowseError::NotFound)?;
        if !node.folder {
            return Err(BrowseError::NotAFolder);
        }
        // Everything whose path starts with "<folder>/", and has no further `/` after that.
        let prefix = if path == "/" { String::from("/") } else { path::join(path, "") };
        let mut items = Vec::new();
        for (key, node) in self.nodes.range(prefix.clone()..) {
            let Some(rest) = key.strip_prefix(prefix.as_str()) else {
                break;
            };
            if !rest.is_empty() && !rest.contains('/') {
                items.push(MemTree::item(key, node));
            }
        }
        Ok(items)
    }

    fn stat(&mut self, path: &str) -> Result<Item, BrowseError> {
        path::tidy(path).ok_or(BrowseError::BadName)?;
        let node = self.nodes.get(path).ok_or(BrowseError::NotFound)?;
        Ok(MemTree::item(path, node))
    }

    /// The checks come in this order: the path, the name, the parent (missing, or a file), the
    /// right to write (any read-only folder above), then whether the name is taken. A name
    /// that is taken in a read-only folder is `NoRight`: the person could not have made it
    /// anyway.
    fn make_folder(&mut self, path: &str) -> Result<(), BrowseError> {
        path::tidy(path).ok_or(BrowseError::BadName)?;
        let parent = path::parent(path).ok_or(BrowseError::AlreadyExists)?;
        path::check_name(path::name(path))?;
        let parent_node = self.nodes.get(parent).ok_or(BrowseError::NotFound)?;
        if !parent_node.folder {
            return Err(BrowseError::NotAFolder);
        }
        let mut above = Some(parent);
        while let Some(folder) = above {
            if self.nodes.get(folder).is_some_and(|n| n.read_only) {
                return Err(BrowseError::NoRight);
            }
            above = path::parent(folder);
        }
        if self.nodes.contains_key(path) {
            return Err(BrowseError::AlreadyExists);
        }
        self.nodes.insert(String::from(path), Node::folder());
        Ok(())
    }
}
