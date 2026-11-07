// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Random operations checked against a model.
//!
//! The model is the simplest thing that could be right: a map from path to contents, held in
//! memory. Each step picks an operation, works out from the model what the answer must be (a
//! success, or one particular error), does it on the real filesystem, and compares. Every so
//! often the volume is unmounted and mounted again, and the whole tree is compared. Because the
//! sequence of random numbers is fixed by the seed, a failure can be run again.

use super::*;
use std::collections::BTreeSet;
use std::string::ToString;

/// Names to pick from. Some differ only in A-Z case (the same name to FAT), some need long-name
/// entries, some are not ASCII.
const NAMES: &[&str] = &[
    "a",
    "A",
    "b",
    "c.txt",
    "C.TXT",
    "Long name with spaces.dat",
    "long NAME with spaces.dat",
    "ünï.txt",
    "日本語.txt",
    "x.y.z",
    "UPPER.TXT",
    "lower.txt",
    "Mixed.Txt",
    "a b",
    "n1",
    "n2",
    "n3",
    "n4",
    "Same Start Of A Name 1",
    "Same Start Of A Name 2",
    "Same Start Of A Name 3",
];

/// Two names are the same to FAT when they differ only in A-Z case.
fn fold(name: &str) -> String {
    name.to_ascii_lowercase()
}

#[derive(Clone, PartialEq, Debug)]
enum Item {
    Folder,
    File(Vec<u8>),
}

/// What the filesystem should hold: full path (folded, starting with `/`) to the name as it is
/// spelt and what it is.
struct Model {
    items: BTreeMap<String, (String, Item)>,
}

impl Model {
    fn name_of(path: &str) -> &str {
        path.rsplit_once('/').map_or(path, |(_, n)| n)
    }
    fn parent_of(path: &str) -> &str {
        path.rsplit_once('/').map_or("", |(p, _)| p)
    }
    fn children(&self, folder: &str) -> Vec<&String> {
        self.items.keys().filter(|k| Self::parent_of(k) == folder && !k.is_empty()).collect()
    }
    fn has_descendants(&self, folder: &str) -> bool {
        let prefix = std::format!("{folder}/");
        self.items.keys().any(|k| k.starts_with(&prefix))
    }
}

struct Run {
    fs: Fat<MemDisk>,
    model: Model,
    rng: Rng,
    /// The id the filesystem gave each path.
    ids: BTreeMap<String, NodeId>,
    /// Ids of nodes that were removed, which must stay `Gone` and must never come back.
    dead: BTreeSet<NodeId>,
    /// Every id ever handed out.
    seen: BTreeSet<NodeId>,
    log: Vec<String>,
}

impl Run {
    fn new(seed: u64, disk: MemDisk) -> Run {
        Run {
            fs: mount(disk),
            model: Model { items: BTreeMap::new() },
            rng: Rng(seed),
            ids: BTreeMap::new(),
            dead: BTreeSet::new(),
            seen: BTreeSet::new(),
            log: Vec::new(),
        }
    }

    fn pick<'a, T>(&mut self, from: &'a [T]) -> Option<&'a T> {
        if from.is_empty() { None } else { Some(&from[self.rng.below(from.len() as u64) as usize]) }
    }

    fn folders(&self) -> Vec<String> {
        let mut all = std::vec![String::new()];
        all.extend(
            self.model.items.iter().filter(|(_, v)| v.1 == Item::Folder).map(|(k, _)| k.clone()),
        );
        all
    }

    fn files(&self) -> Vec<String> {
        self.model
            .items
            .iter()
            .filter(|(_, v)| matches!(v.1, Item::File(_)))
            .map(|(k, _)| k.clone())
            .collect()
    }

    fn node(&mut self, path: &str) -> NodeId {
        if path.is_empty() {
            return self.fs.root();
        }
        let id = node_at(&mut self.fs, path).unwrap_or_else(|e| panic!("{path}: {e:?}"));
        if let Some(&known) = self.ids.get(path) {
            assert_eq!(id, known, "{path}: a node keeps its id");
        } else {
            assert!(!self.dead.contains(&id), "{path}: the id of a removed node came back");
            self.ids.insert(path.to_string(), id);
        }
        self.seen.insert(id);
        id
    }

    fn step(&mut self) {
        let op = self.rng.below(100);
        let folders = self.folders();
        let files = self.files();
        let note = |run: &mut Run, s: String| run.log.push(s);
        match op {
            // Create a file or a folder.
            0..=24 => {
                let parent = self.pick(&folders).unwrap().clone();
                let name = *self.pick(NAMES).unwrap();
                let path = std::format!("{parent}/{}", fold(name));
                let kind = if self.rng.below(4) == 0 { Kind::Folder } else { Kind::File };
                // Keep the tree shallow.
                if kind == Kind::Folder && parent.matches('/').count() >= 3 {
                    return;
                }
                note(self, std::format!("create {path} {kind:?}"));
                let pid = self.node(&parent);
                let got = self.fs.create(pid, name, kind);
                if self.model.items.contains_key(&path) {
                    assert_eq!(got, Err(FsError::AlreadyExists), "{path}");
                } else {
                    let id = got.unwrap_or_else(|e| panic!("create {path}: {e:?}"));
                    assert!(self.seen.insert(id), "a new node reuses id {id}");
                    self.ids.insert(path.clone(), id);
                    let item =
                        if kind == Kind::Folder { Item::Folder } else { Item::File(Vec::new()) };
                    self.model.items.insert(path, (name.to_string(), item));
                }
            }
            // Write into a file, sometimes far past its end.
            25..=49 => {
                let Some(path) = self.pick(&files).cloned() else { return };
                let (offset, len) = match self.rng.below(4) {
                    0 => (0, self.rng.below(100)),
                    1 => (self.rng.below(3000), self.rng.below(3000)),
                    2 => (self.rng.below(30_000), self.rng.below(2000)),
                    _ => (0, self.rng.below(20_000)),
                };
                let data = bytes(self.rng.next(), len as usize);
                note(self, std::format!("write {path} at {offset} len {len}"));
                let id = self.node(&path);
                assert_eq!(self.fs.write(id, offset, &data), Ok(data.len()));
                let Item::File(content) = &mut self.model.items.get_mut(&path).unwrap().1 else {
                    unreachable!()
                };
                if !data.is_empty() {
                    let end = (offset as usize) + data.len();
                    if content.len() < end {
                        content.resize(end, 0);
                    }
                    content[offset as usize..end].copy_from_slice(&data);
                }
            }
            // Cut or extend.
            50..=57 => {
                let Some(path) = self.pick(&files).cloned() else { return };
                let size = self.rng.below(25_000);
                note(self, std::format!("set_size {path} {size}"));
                let id = self.node(&path);
                self.fs.set_size(id, size).unwrap();
                let Item::File(content) = &mut self.model.items.get_mut(&path).unwrap().1 else {
                    unreachable!()
                };
                content.resize(size as usize, 0);
            }
            58..=65 => {
                let Some(path) = self.pick(&files).cloned() else { return };
                note(self, std::format!("commit {path}"));
                let id = self.node(&path);
                self.fs.commit(id).unwrap();
            }
            // Remove.
            66..=77 => {
                let all: Vec<String> = self.model.items.keys().cloned().collect();
                let Some(path) = self.pick(&all).cloned() else { return };
                note(self, std::format!("remove {path}"));
                let id = self.node(&path);
                let parent = self.node(Model::parent_of(&path).to_string().as_str());
                let name = Model::name_of(&path).to_string();
                let got = self.fs.remove(parent, &name);
                if self.model.has_descendants(&path) {
                    assert_eq!(got, Err(FsError::FolderNotEmpty), "{path}");
                } else {
                    got.unwrap_or_else(|e| panic!("remove {path}: {e:?}"));
                    self.model.items.remove(&path);
                    self.ids.remove(&path);
                    self.dead.insert(id);
                    assert_eq!(self.fs.stat(id), Err(FsError::Gone));
                }
            }
            // Rename or move, sometimes replacing.
            78..=92 => {
                let all: Vec<String> = self.model.items.keys().cloned().collect();
                let Some(from) = self.pick(&all).cloned() else { return };
                let to_parent = self.pick(&folders).unwrap().clone();
                let name = *self.pick(NAMES).unwrap();
                let to = std::format!("{to_parent}/{}", fold(name));
                let replace = self.rng.below(2) == 0;
                note(self, std::format!("rename {from} -> {to} replace={replace}"));
                let id = self.node(&from);
                let from_parent = self.node(Model::parent_of(&from).to_string().as_str());
                let to_parent_id = self.node(&to_parent);
                let from_name = Model::name_of(&from).to_string();
                let got = self.fs.rename(from_parent, &from_name, to_parent_id, name, replace);

                let into_itself = to == from || to.starts_with(&std::format!("{from}/"));
                let dest = self.model.items.get(&to).cloned();
                let renamed_in_place = from == to;
                if into_itself && !renamed_in_place {
                    assert_eq!(got, Err(FsError::BadArgument), "{from} -> {to}");
                    return;
                }
                if renamed_in_place {
                    // Same name: either nothing changes or only the spelling does.
                    got.unwrap_or_else(|e| panic!("{from} -> {to}: {e:?}"));
                    self.model.items.get_mut(&from).unwrap().0 = name.to_string();
                    return;
                }
                match dest {
                    Some((_, Item::Folder)) => {
                        assert_eq!(got, Err(FsError::AlreadyExists), "{from} -> {to}")
                    }
                    Some((_, Item::File(_))) if !replace => {
                        assert_eq!(got, Err(FsError::AlreadyExists), "{from} -> {to}")
                    }
                    other => {
                        got.unwrap_or_else(|e| panic!("{from} -> {to}: {e:?}"));
                        if other.is_some() {
                            // The file that was replaced is gone.
                            let old = self.ids.remove(&to);
                            if let Some(old) = old {
                                self.dead.insert(old);
                                assert_eq!(self.fs.stat(old), Err(FsError::Gone));
                            }
                            self.model.items.remove(&to);
                        }
                        // Move the item, and everything under it if it is a folder.
                        let prefix = std::format!("{from}/");
                        let moving: Vec<String> = self
                            .model
                            .items
                            .keys()
                            .filter(|k| **k == from || k.starts_with(&prefix))
                            .cloned()
                            .collect();
                        for old in moving {
                            let (spelt, item) = self.model.items.remove(&old).unwrap();
                            let new = std::format!("{to}{}", &old[from.len()..]);
                            let spelt = if old == from { name.to_string() } else { spelt };
                            self.model.items.insert(new.clone(), (spelt, item));
                            if let Some(id) = self.ids.remove(&old) {
                                self.ids.insert(new, id);
                            }
                        }
                        // The renamed node keeps its id.
                        assert_eq!(node_at(&mut self.fs, &to), Ok(id), "{to}: id after rename");
                    }
                }
            }
            // Look at things: read a file, list a folder.
            93..=97 => {
                if let Some(path) = self.pick(&files).cloned() {
                    self.check_file(&path);
                }
                let folder = self.pick(&folders).unwrap().clone();
                self.check_folder(&folder);
            }
            // Unmount and mount again.
            _ => {
                note(self, "remount".to_string());
                self.remount();
            }
        }
    }

    fn check_file(&mut self, path: &str) {
        let id = self.node(path);
        let Item::File(want) = self.model.items[path].1.clone() else { unreachable!() };
        assert_eq!(self.fs.stat(id).unwrap().size, want.len() as u64, "{path}: size");
        assert!(read_all(&mut self.fs, id).unwrap() == want, "{path}: contents");
        // And a read in the middle, past the end and across the end.
        if !want.is_empty() {
            let at = self.rng.below(want.len() as u64) as usize;
            let mut buf = std::vec![0u8; 700];
            let n = self.fs.read(id, at as u64, &mut buf).unwrap();
            assert_eq!(n, 700.min(want.len() - at));
            assert!(buf[..n] == want[at..at + n]);
        }
        let mut buf = [0u8; 4];
        assert_eq!(self.fs.read(id, want.len() as u64 + 10, &mut buf), Ok(0));
    }

    fn check_folder(&mut self, folder: &str) {
        let id = self.node(folder);
        let mut got: Vec<(String, Kind, u64)> =
            list_all(&mut self.fs, id).into_iter().map(|e| (e.name, e.kind, e.size)).collect();
        got.sort_by_key(|g| (g.0.clone(), g.1 == Kind::Folder, g.2));
        let mut want: Vec<(String, Kind, u64)> = self
            .model
            .children(folder)
            .into_iter()
            .map(|p| {
                let (name, item) = &self.model.items[p];
                match item {
                    Item::Folder => (name.clone(), Kind::Folder, 0),
                    Item::File(c) => (name.clone(), Kind::File, c.len() as u64),
                }
            })
            .collect();
        want.sort_by_key(|g| (g.0.clone(), g.1 == Kind::Folder, g.2));
        assert_eq!(got, want, "listing of {folder:?}");
        // The names the folder gives out in pages are the same names.
        let mut paged = names(&mut self.fs, id);
        paged.sort();
        let mut want_names: Vec<String> = want.into_iter().map(|w| w.0).collect();
        want_names.sort();
        assert_eq!(paged, want_names, "paged listing of {folder:?}");
    }

    /// Compares the whole volume with the model.
    fn check_all(&mut self) {
        let mut folders = self.folders();
        folders.sort();
        for f in folders {
            self.check_folder(&f);
        }
        for f in self.files() {
            self.check_file(&f);
        }
    }

    fn remount(&mut self) {
        self.fs.unmount().unwrap();
        // Move the disk's bytes into a new disk, and mount that: nothing is kept in memory
        // from the old mount.
        let data = core::mem::take(&mut self.fs.device_mut().data);
        let disk = MemDisk::from(data);
        self.fs = mount(disk);
        // Ids are for one mount only.
        self.ids.clear();
        self.dead.clear();
        self.seen.clear();
        self.check_all();
    }
}

fn run(seed: u64, steps: usize) {
    let mut run = Run::new(seed, blank_with(70_000, 1, "MODEL"));
    for i in 0..steps {
        let log_len = run.log.len();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run.step()));
        if let Err(e) = result {
            let recent = &run.log[run.log.len().saturating_sub(8)..];
            let _ = log_len;
            panic!("seed {seed}, step {i}; the last operations: {recent:#?}\n{e:?}");
        }
    }
    run.check_all();
    run.remount();
}

#[test]
fn random_operations_agree_with_the_model() {
    for seed in 1..=4 {
        run(seed * 0x9E37_79B9 + 1, 500);
    }
}

#[test]
fn many_remounts() {
    // The same, with a remount after nearly every step, so every change must reach the disk.
    let mut run = Run::new(77, blank_with(70_000, 1, "MODEL"));
    for _ in 0..150 {
        run.step();
        run.remount();
    }
}
