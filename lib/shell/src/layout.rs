// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Paths, and the shape of the shell's file tree.
//!
//! The shell sees its session's tree (note 0014, section 8; note 0017, section 3): a list of
//! **entries**, each a path such as `/Home` or `/Volumes/Data` with a capability (a right to use
//! one folder or file, held by the kernel for the program; note 0006) behind it. Everything
//! inside an entry is real and the filesystem answers for it. The folders *above* the entries
//! (`/`, `/Volumes`, `/System`) exist only because something is attached below them: they list
//! what is below and can't be changed. [`Layout`] knows which is which, so the rest of the shell
//! can ask "what is at this path?" before it asks a filesystem anything.
//!
//! The plain path work (making a path absolute, resolving `..`) is `pbr_filetree::normalize`,
//! the same code the program's own file library uses, so the shell and the tree always agree.
//! `..` is resolved here by text, and is never sent to a filesystem, so it can't climb out of
//! an entry.

use crate::fs::Item;
use crate::reason::Reason;
use alloc::string::String;
use alloc::vec::Vec;
use pbr_filetree::{Found, Kind, Mount, Tree};

/// The folder `cd` goes to when given no path, and where the shell starts.
pub const HOME: &str = "/Home";

/// The longest path the shell handles, in bytes (the file protocol's limit, note 0014).
pub const MAX_PATH: usize = pbr_filetree::MAX_PATH;

/// Makes `typed` an absolute, tidy path: relative paths start at `cwd` (which must already be
/// absolute and tidy), `.` and empty names go, `..` removes the name before it (and stays at
/// `/`). The result is `/` or `/name/name…`.
pub fn resolve(cwd: &str, typed: &str) -> Result<String, Reason> {
    pbr_filetree::normalize(cwd, typed).map_err(|_| Reason::BadPath)
}

/// The folder above `path` (`/` for a top-level name, and for `/` itself).
pub fn parent(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) | None => "/",
        Some(i) => &path[..i],
    }
}

/// The last name in `path`, or `None` for `/`.
pub fn base_name(path: &str) -> Option<&str> {
    match path.rfind('/') {
        Some(i) if i + 1 < path.len() => Some(&path[i + 1..]),
        _ => None,
    }
}

/// The path of `name` inside the folder `folder`.
pub fn join(folder: &str, name: &str) -> String {
    if folder == "/" { alloc::format!("/{name}") } else { alloc::format!("{folder}/{name}") }
}

/// Whether `path` is `folder` itself or something inside it (both absolute and tidy).
pub fn is_within(folder: &str, path: &str) -> bool {
    folder == "/"
        || path == folder
        || (path.len() > folder.len()
            && path.starts_with(folder)
            && path.as_bytes()[folder.len()] == b'/')
}

/// What is at a path, as far as the tree's shape can tell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    /// An entry itself: the top of an attached folder or file.
    Mount {
        /// Which entry.
        entry: usize,
        /// A folder, not a file.
        folder: bool,
    },
    /// Inside an entry: the filesystem knows whether it exists.
    Inside {
        /// Which entry.
        entry: usize,
    },
    /// A folder that exists only because entries are attached below it. These are the names
    /// directly in it.
    Virtual(Vec<Item>),
    /// Nothing the shell holds is here.
    Nowhere,
}

/// The shape of the tree: where things are attached.
#[derive(Clone, Debug, Default)]
pub struct Layout {
    tree: Tree,
}

impl Layout {
    /// The layout of a tree with these entries (path, whether a folder), as
    /// `pbr_files::Files::entries` lists them. Entries that break the tree's rules are left
    /// out: the program's own file library has checked them already, so this only keeps the
    /// shell safe from a surprise.
    pub fn new(entries: &[(String, bool)]) -> Layout {
        let mounts: Vec<Mount> = entries
            .iter()
            .map(|(path, folder)| Mount {
                path: path.clone(),
                kind: if *folder { Kind::Folder } else { Kind::File },
            })
            .collect();
        Layout { tree: Tree::new(mounts).unwrap_or_default() }
    }

    /// What is at `path` (absolute and tidy).
    pub fn place(&self, path: &str) -> Place {
        match self.tree.find(path) {
            Found::Entry { index, rest: "" } => Place::Mount {
                entry: index,
                folder: self.tree.mounts()[index].kind == Kind::Folder,
            },
            Found::Entry { index, .. } => Place::Inside { entry: index },
            Found::Above(children) => Place::Virtual(
                children
                    .into_iter()
                    .map(|(name, kind)| Item { name, folder: kind == Kind::Folder, size: 0 })
                    .collect(),
            ),
            Found::Nothing => Place::Nowhere,
        }
    }

    /// Whether both paths are in (or are) the same entry, so a filesystem can rename between
    /// them in one step.
    pub fn same_entry(&self, a: &str, b: &str) -> bool {
        let entry = |p: &str| match self.place(p) {
            Place::Mount { entry, .. } | Place::Inside { entry } => Some(entry),
            Place::Virtual(_) | Place::Nowhere => None,
        };
        matches!((entry(a), entry(b)), (Some(x), Some(y)) if x == y)
    }

    /// The entries that are whole volumes' worth of space to report: the home folder and each
    /// folder attached directly under `/Volumes`, sorted by path.
    pub fn volume_roots(&self) -> Vec<String> {
        let mut roots: Vec<String> = self
            .tree
            .mounts()
            .iter()
            .filter(|m| m.kind == Kind::Folder)
            .filter(|m| m.path == HOME || parent(&m.path) == "/Volumes")
            .map(|m| m.path.clone())
            .collect();
        roots.sort();
        roots
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;

    fn layout() -> Layout {
        let e = |p: &str, f: bool| (p.to_string(), f);
        Layout::new(&[
            e("/Home", true),
            e("/System/rules.txt", false),
            e("/Volumes/Data", true),
            e("/Volumes/Old disk", true),
        ])
    }

    #[test]
    fn resolving_paths() {
        for (cwd, typed, want) in [
            ("/Home", "a.txt", "/Home/a.txt"),
            ("/Home", "/System/rules.txt", "/System/rules.txt"),
            ("/Home", "..", "/"),
            ("/Home", "../../../..", "/"),
            ("/Home/x", "./../y/./z/", "/Home/y/z"),
            ("/", "", "/"),
            ("/Home", ".", "/Home"),
            ("/Home", "a//b", "/Home/a/b"),
            ("/Home", "/..//Home/../Volumes", "/Volumes"),
        ] {
            assert_eq!(resolve(cwd, typed).as_deref(), Ok(want), "{cwd} + {typed}");
        }
    }

    #[test]
    fn hostile_paths() {
        // Control characters and too-long names or paths are refused, not mangled.
        assert_eq!(resolve("/", "a\u{7}b"), Err(Reason::BadPath));
        assert_eq!(resolve("/", &"a".repeat(256)), Err(Reason::BadPath));
        let long = vec!["abcdefghij"; 500].join("/");
        assert_eq!(resolve("/", &long), Err(Reason::BadPath));
        // A path full of `..` never leaves the top.
        let ups = vec![".."; 5000].join("/");
        assert_eq!(resolve("/Home", &ups), Err(Reason::BadPath));
        let ups = vec![".."; 1000].join("/");
        assert_eq!(resolve("/Home", &ups).as_deref(), Ok("/"));
        // Climbing out and walking back in lands on the same absolute path.
        assert_eq!(resolve("/Home/a/b", "../../../Home/a").as_deref(), Ok("/Home/a"));
    }

    #[test]
    fn path_pieces() {
        assert_eq!(parent("/Home/a"), "/Home");
        assert_eq!(parent("/Home"), "/");
        assert_eq!(parent("/"), "/");
        assert_eq!(base_name("/Home/a.txt"), Some("a.txt"));
        assert_eq!(base_name("/Home"), Some("Home"));
        assert_eq!(base_name("/"), None);
        assert_eq!(join("/", "Home"), "/Home");
        assert_eq!(join("/Home", "a"), "/Home/a");
    }

    #[test]
    fn within() {
        assert!(is_within("/Home", "/Home"));
        assert!(is_within("/Home", "/Home/a/b"));
        assert!(!is_within("/Home", "/Homework"));
        assert!(!is_within("/Home/a", "/Home"));
        assert!(is_within("/", "/anything"));
        assert!(is_within("/", "/"));
    }

    #[test]
    fn what_is_where() {
        let l = layout();
        assert_eq!(l.place("/Home"), Place::Mount { entry: 0, folder: true });
        assert_eq!(l.place("/Home/a/b"), Place::Inside { entry: 0 });
        assert_eq!(l.place("/System/rules.txt"), Place::Mount { entry: 1, folder: false });
        assert_eq!(l.place("/Nothing"), Place::Nowhere);
        assert_eq!(l.place("/System/other"), Place::Nowhere);
        assert_eq!(l.place("/Homes"), Place::Nowhere);
    }

    #[test]
    fn folders_that_only_hold_folders() {
        let l = layout();
        let names = |p: &str| -> Vec<(String, bool)> {
            match l.place(p) {
                Place::Virtual(items) => items.into_iter().map(|i| (i.name, i.folder)).collect(),
                other => panic!("{p}: {other:?}"),
            }
        };
        assert_eq!(
            names("/"),
            vec![
                ("Home".to_string(), true),
                ("System".to_string(), true),
                ("Volumes".to_string(), true)
            ]
        );
        assert_eq!(names("/System"), vec![("rules.txt".to_string(), false)]);
        assert_eq!(
            names("/Volumes"),
            vec![("Data".to_string(), true), ("Old disk".to_string(), true)]
        );
        // A standard user has no volumes: /Volumes isn't there at all.
        let standard = Layout::new(&[("/Home".to_string(), true)]);
        assert_eq!(standard.place("/Volumes"), Place::Nowhere);
        assert!(matches!(standard.place("/"), Place::Virtual(v) if v.len() == 1));
        // A session with no files at all still has a root.
        assert_eq!(Layout::new(&[]).place("/"), Place::Virtual(Vec::new()));
    }

    #[test]
    fn same_entry() {
        let l = layout();
        assert!(l.same_entry("/Home/a", "/Home/b/c"));
        assert!(l.same_entry("/Home", "/Home/b"));
        assert!(!l.same_entry("/Home/a", "/Volumes/Data/a"));
        assert!(!l.same_entry("/Volumes/Data/a", "/Volumes/Old disk/a"));
        assert!(!l.same_entry("/Home/a", "/Volumes"));
        assert!(!l.same_entry("/Home/a", "/Nowhere"));
    }

    #[test]
    fn volume_roots() {
        assert_eq!(
            layout().volume_roots(),
            vec!["/Home".to_string(), "/Volumes/Data".to_string(), "/Volumes/Old disk".to_string()]
        );
        assert!(Layout::new(&[]).volume_roots().is_empty());
    }

    #[test]
    fn broken_entries_leave_an_empty_layout() {
        // Nested entries break the tree's rules; the shell then sees nothing rather than guess.
        let l = Layout::new(&[("/Home".to_string(), true), ("/Home/x".to_string(), true)]);
        assert_eq!(l.place("/Home"), Place::Nowhere);
        assert_eq!(l.place("/"), Place::Virtual(Vec::new()));
    }
}
