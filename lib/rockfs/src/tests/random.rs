// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Random operations checked against a simple model, over many seeds, with commits, clean
//! remounts and crashes (remounting without unmounting).

use super::*;
use std::collections::BTreeMap;

/// What the model knows about one file or folder.
struct Item {
    /// The name as it was given when created or renamed.
    name: String,
    kind: Kind,
    /// The file's contents including changes not yet committed.
    current: Vec<u8>,
    /// The contents as of its last commit: what a crash leaves.
    committed: Vec<u8>,
    id: NodeId,
}

/// A model of the whole tree: lower-cased path (A-Z only) to its item.
#[derive(Default)]
struct Model {
    items: BTreeMap<String, Item>,
}

fn fold(s: &str) -> String {
    s.chars().map(|c| c.to_ascii_lowercase()).collect()
}

impl Model {
    fn folders(&self) -> Vec<String> {
        let mut v = std::vec![String::new()];
        v.extend(self.items.iter().filter(|(_, i)| i.kind == Kind::Folder).map(|(k, _)| k.clone()));
        v
    }
    fn files(&self) -> Vec<String> {
        self.items.iter().filter(|(_, i)| i.kind == Kind::File).map(|(k, _)| k.clone()).collect()
    }
    fn children(&self, folder: &str) -> usize {
        let prefix = std::format!("{folder}/");
        self.items.keys().filter(|k| k.starts_with(&prefix)).count()
    }
}

fn join(folder: &str, name: &str) -> String {
    std::format!("{folder}/{name}")
}

const NAMES: &[&str] =
    &["a", "B", "c.txt", "D.TXT", "e", "A very long name with spaces .txt", "É", "é", "x1", "X2"];

/// Walks the volume and checks it holds exactly what the model says.
fn verify(fs: &mut RockFs<MemDisk>, model: &Model, what: &str) {
    let mut found = 0;
    let mut stack = std::vec![(String::new(), fs.root())];
    while let Some((path, folder)) = stack.pop() {
        let mut entries = Vec::new();
        let mut cursor = 0;
        loop {
            cursor = fs
                .list(folder, cursor, &mut |e| {
                    entries.push(e);
                    true
                })
                .unwrap();
            if cursor == 0 {
                break;
            }
        }
        for e in entries {
            let key = join(&path, &fold(&e.name));
            let item = model.items.get(&key).unwrap_or_else(|| panic!("{what}: unexpected {key}"));
            assert_eq!(item.name, e.name, "{what}: spelling of {key}");
            assert_eq!(item.kind, e.kind, "{what}: kind of {key}");
            let id = fs.lookup(folder, &e.name).unwrap();
            assert_eq!(id, item.id, "{what}: node id of {key}");
            found += 1;
            if e.kind == Kind::Folder {
                stack.push((key, id));
            } else {
                assert_eq!(e.size, item.current.len() as u64, "{what}: size of {key}");
                assert_eq!(read_all(fs, id).unwrap(), item.current, "{what}: contents of {key}");
            }
        }
    }
    assert_eq!(found, model.items.len(), "{what}: item count");
}

fn run(seed: u64, ops: usize, blocks: u64) {
    let mut rng = Rng(seed * 7919 + 13);
    let mut fs = fresh(blocks);
    let mut model = Model::default();
    for step in 0..ops {
        let what = std::format!("seed {seed} step {step}");
        let op = rng.below(100);
        let folders = model.folders();
        let folder_key = folders[rng.below(folders.len() as u64) as usize].clone();
        let folder_id = if folder_key.is_empty() { fs.root() } else { model.items[&folder_key].id };
        let name = NAMES[rng.below(NAMES.len() as u64) as usize];
        // A random spelling of the name, to exercise case-insensitive matching.
        let spelled: String = name
            .chars()
            .map(|c| if rng.below(2) == 0 { c.to_ascii_uppercase() } else { c })
            .collect();
        let key = join(&folder_key, &fold(name));
        match op {
            0..=24 => {
                // Create a file or folder.
                let kind = if rng.below(5) == 0 { Kind::Folder } else { Kind::File };
                let r = fs.create(folder_id, &spelled, kind);
                if model.items.contains_key(&key) {
                    assert_eq!(r, Err(FsError::AlreadyExists), "{what}");
                } else if model.children(&folder_key) < 40 || kind == Kind::File {
                    match r {
                        Ok(id) => {
                            model.items.insert(
                                key,
                                Item {
                                    name: spelled,
                                    kind,
                                    current: Vec::new(),
                                    committed: Vec::new(),
                                    id,
                                },
                            );
                        }
                        Err(FsError::NoSpace) => {}
                        Err(e) => panic!("{what}: create failed: {e:?}"),
                    }
                }
            }
            25..=49 => {
                // Write into a file.
                let files = model.files();
                if files.is_empty() {
                    continue;
                }
                let k = &files[rng.below(files.len() as u64) as usize];
                let item = model.items.get_mut(k).unwrap();
                let off = match rng.below(4) {
                    0 => 0,
                    1 => item.current.len(),
                    _ => rng.below(item.current.len() as u64 + 6000) as usize,
                };
                let len = match rng.below(5) {
                    0 => 1,
                    1 => 4096,
                    2 => 4095 + rng.below(3) as usize,
                    _ => rng.below(20_000) as usize,
                };
                let data = bytes(rng.next(), len);
                match fs.write(item.id, off as u64, &data) {
                    Ok(n) => {
                        assert_eq!(n, len, "{what}");
                        if len > 0 {
                            if item.current.len() < off + len {
                                item.current.resize(off + len, 0);
                            }
                            item.current[off..off + len].copy_from_slice(&data);
                        }
                    }
                    Err(FsError::NoSpace) => {}
                    Err(e) => panic!("{what}: write failed: {e:?}"),
                }
            }
            50..=54 => {
                let files = model.files();
                if files.is_empty() {
                    continue;
                }
                let k = &files[rng.below(files.len() as u64) as usize];
                let item = model.items.get_mut(k).unwrap();
                let size = rng.below(item.current.len() as u64 + 9000) as usize;
                match fs.set_size(item.id, size as u64) {
                    Ok(()) => item.current.resize(size, 0),
                    Err(FsError::NoSpace) => {}
                    Err(e) => panic!("{what}: set_size failed: {e:?}"),
                }
            }
            55..=64 => {
                // Commit one file.
                let files = model.files();
                if files.is_empty() {
                    continue;
                }
                let k = &files[rng.below(files.len() as u64) as usize];
                let item = model.items.get_mut(k).unwrap();
                match fs.commit(item.id) {
                    Ok(()) => item.committed = item.current.clone(),
                    Err(FsError::NoSpace) => {}
                    Err(e) => panic!("{what}: commit failed: {e:?}"),
                }
            }
            65..=74 => {
                // Remove.
                let r = fs.remove(folder_id, &spelled);
                match model.items.get(&key) {
                    None => assert_eq!(r, Err(FsError::NotFound), "{what}"),
                    Some(item) if item.kind == Kind::Folder && model.children(&key) > 0 => {
                        assert_eq!(r, Err(FsError::FolderNotEmpty), "{what}")
                    }
                    Some(item) => {
                        let id = item.id;
                        r.unwrap_or_else(|e| panic!("{what}: remove failed: {e:?}"));
                        model.items.remove(&key);
                        assert_eq!(fs.stat(id), Err(FsError::Gone), "{what}");
                    }
                }
            }
            75..=89 => {
                // Rename, into a random folder, with or without replace.
                let all: Vec<String> = model.items.keys().cloned().collect();
                if all.is_empty() {
                    continue;
                }
                let src_key = all[rng.below(all.len() as u64) as usize].clone();
                let (src_folder, src_name) = src_key.rsplit_once('/').unwrap();
                let src_folder_id =
                    if src_folder.is_empty() { fs.root() } else { model.items[src_folder].id };
                let replace = rng.below(2) == 0;
                let dst_key = join(&folder_key, &fold(&spelled));
                let moving_kind = model.items[&src_key].kind;
                let into_itself = moving_kind == Kind::Folder
                    && (folder_key == src_key
                        || folder_key.starts_with(&std::format!("{src_key}/")));
                let r = fs.rename(src_folder_id, src_name, folder_id, &spelled, replace);
                let existing = model.items.get(&dst_key).map(|i| i.kind);
                let expect_err = if into_itself {
                    Some(FsError::BadArgument)
                } else if dst_key == src_key {
                    None
                } else if existing.is_some() && !(replace && existing == Some(Kind::File)) {
                    Some(FsError::AlreadyExists)
                } else {
                    None
                };
                match (r, expect_err) {
                    (Err(e), Some(want)) => assert_eq!(e, want, "{what}"),
                    (Err(FsError::NoSpace), None) => {}
                    (Err(e), None) => panic!("{what}: rename failed: {e:?}"),
                    (Ok(()), Some(want)) => panic!("{what}: rename should fail with {want:?}"),
                    (Ok(()), None) => {
                        let mut item = model.items.remove(&src_key).unwrap();
                        // Everything below a moved folder moves with it.
                        let below: Vec<String> = model
                            .items
                            .keys()
                            .filter(|k| k.starts_with(&std::format!("{src_key}/")))
                            .cloned()
                            .collect();
                        let moved: Vec<(String, Item)> = below
                            .into_iter()
                            .map(|k| {
                                let it = model.items.remove(&k).unwrap();
                                (String::from(&k[src_key.len()..]), it)
                            })
                            .collect();
                        if let Some(old) = model.items.remove(&dst_key) {
                            assert_eq!(fs.stat(old.id), Err(FsError::Gone), "{what}");
                        }
                        // On replacement the new spelling is kept (unless it doesn't fit, which
                        // can't happen with these short names), and a case-only rename shows it.
                        item.name = spelled.clone();
                        model.items.insert(dst_key.clone(), item);
                        for (rest, it) in moved {
                            model.items.insert(std::format!("{dst_key}{rest}"), it);
                        }
                    }
                }
            }
            90..=93 => {
                // A clean restart: everything is committed.
                fs.unmount().unwrap();
                fs = RockFs::mount(fs.into_device(), clock, false).unwrap();
                for item in model.items.values_mut() {
                    item.committed = item.current.clone();
                }
                assert!(fs.mount_report().is_empty(), "{what}");
                verify(&mut fs, &model, &what);
            }
            94..=96 => {
                // A crash: nothing is unmounted, so uncommitted changes are lost.
                fs = RockFs::mount(fs.into_device(), clock, false).unwrap();
                assert!(
                    fs.mount_report().iter().all(|p| p.harmless),
                    "{what}: {:?}",
                    fs.mount_report()
                );
                for item in model.items.values_mut() {
                    item.current = item.committed.clone();
                }
                verify(&mut fs, &model, &what);
                assert_clean(&mut fs);
            }
            _ => {
                // Look at a file by a differently-spelled name.
                let files = model.files();
                if files.is_empty() {
                    continue;
                }
                let k = &files[rng.below(files.len() as u64) as usize];
                let (f, n) = k.rsplit_once('/').unwrap();
                let fid = if f.is_empty() { fs.root() } else { model.items[f].id };
                let upper: String = n.chars().map(|c| c.to_ascii_uppercase()).collect();
                assert_eq!(fs.lookup(fid, &upper).unwrap(), model.items[k].id, "{what}");
            }
        }
        if step % 50 == 49 {
            verify(&mut fs, &model, &what);
        }
    }
    let what = std::format!("seed {seed} end");
    verify(&mut fs, &model, &what);
    assert_clean(&mut fs);
    fs.unmount().unwrap();
    let mut fs = RockFs::mount(fs.into_device(), clock, false).unwrap();
    verify(&mut fs, &model, &what);
    assert_clean(&mut fs);
}

#[test]
fn random_operations_on_a_roomy_volume() {
    for seed in 0..24 {
        run(seed, 250, 2048);
    }
}

#[test]
fn random_operations_on_a_cramped_volume() {
    // 160 blocks: the disk fills up now and then, so NoSpace is met with changes in flight.
    for seed in 100..124 {
        run(seed, 250, 160);
    }
}
