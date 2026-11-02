// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! `stress` and `verify`: random changes to a volume with a manifest of what should result,
//! and a check of a volume against such a manifest. `tools/rockfs_cross.py` uses them to
//! make this implementation, `tools/rockfs.py` and the C implementation test one another.
//!
//! A *manifest* is a text file with one line per file or folder:
//! `F LENGTH CRC32 PATH` or `D 0 00000000 PATH`, the CRC-32 in eight hexadecimal digits and
//! the path starting with `/` (it may contain spaces: it is the rest of the line).
//! `verify` also accepts the lines `LENGTH CRC32 PATH` that
//! `classic/tools/hosttest/rfs_host verify` reads.

use crate::commands::{Outcome, Volume, close, mount};
use crate::tree::{explain, read_file, resolve, resolve_parent, walk, write_at};
use crate::util::describe;
use pbr_fs::{Filesystem, FsError, Kind};
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// A file or folder the model says is on the volume.
#[derive(Clone)]
struct Item {
    /// Its path, spelled as it was created.
    path: String,
    folder: bool,
    len: u64,
    crc: u32,
}

/// The model: what the volume should hold, by lower-case path (names match with A-Z folded).
#[derive(Default)]
struct Model {
    items: BTreeMap<String, Item>,
}

fn key(path: &str) -> String {
    path.to_ascii_lowercase()
}

impl Model {
    fn files(&self) -> Vec<String> {
        self.items.iter().filter(|(_, i)| !i.folder).map(|(k, _)| k.clone()).collect()
    }
    fn folders(&self) -> Vec<String> {
        self.items.iter().filter(|(_, i)| i.folder).map(|(k, _)| k.clone()).collect()
    }
    fn is_empty_folder(&self, k: &str) -> bool {
        let prefix = format!("{k}/");
        !self.items.keys().any(|o| o.starts_with(&prefix))
    }
    fn insert(&mut self, item: Item) {
        self.items.insert(key(&item.path), item);
    }
}

/// A small, fast random number generator (xorshift64*), so a seed always gives the same run.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        // Mixing the seed in avoids the all-zero state and makes nearby seeds differ a lot.
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0x2545_F491_4F6C_DD1D | 1)
    }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        (self.next() >> 11) % n.max(1)
    }
    fn pick<'a, T>(&mut self, list: &'a [T]) -> &'a T {
        &list[self.below(list.len() as u64) as usize]
    }
}

/// Deterministic file contents: the same `(seed, len)` always gives the same bytes.
fn contents(seed: u64, len: usize) -> Vec<u8> {
    let mut x = seed | 1;
    (0..len)
        .map(|_| {
            x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            (x >> 33) as u8
        })
        .collect()
}

/// Sizes in bytes: mostly small, some across block edges, a few of several hundred KiB (so
/// that fragmenting the volume makes extent trees).
const SIZES: [usize; 14] =
    [0, 1, 100, 4095, 4096, 4097, 9000, 30_000, 70_000, 200_000, 350_000, 17, 5000, 12_288];

/// Names that exercise the rules: spaces, capitals, and non-ASCII letters.
fn file_name(r: &mut Rng) -> String {
    let n = r.below(60);
    match r.below(10) {
        0 => format!("FILE {n}.TXT"),
        1 => format!("Café {n}.txt"),
        2 => format!("日本 {n}.dat"),
        3 => format!("Moved {n}.TXT"),
        _ => format!("File {n}.txt"),
    }
}

fn folder_name(r: &mut Rng) -> String {
    let n = r.below(30);
    if r.below(8) == 0 { format!("FOLDER {n}") } else { format!("Folder {n}") }
}

/// Reads the whole tree into a model (sizes and checksums), so a run can start from whatever
/// the volume already holds.
fn scan(fs: &mut Volume) -> Result<Model, String> {
    let mut list = Vec::new();
    let root = fs.root();
    walk(fs, root, "", &mut list)?;
    let mut model = Model::default();
    for (path, kind, size) in list {
        let folder = kind == Kind::Folder;
        let crc = if folder {
            0
        } else {
            let node = resolve(fs, &path)?;
            pbr_crc::crc32(&read_file(fs, node).map_err(|e| explain(&path, e))?)
        };
        model.insert(Item { path, folder, len: if folder { 0 } else { size }, crc });
    }
    Ok(model)
}

/// Checks every file's contents and every folder against the model.
fn verify_model(fs: &mut Volume, model: &Model, when: &str) -> Result<(), String> {
    for item in model.items.values() {
        let node = resolve(fs, &item.path).map_err(|e| format!("{when}: {e}"))?;
        let st = fs.stat(node).map_err(|e| format!("{when}: {}", explain(&item.path, e)))?;
        if item.folder != (st.kind == Kind::Folder) {
            return Err(format!("{when}: {} has the wrong kind", item.path));
        }
        if item.folder {
            continue;
        }
        let data =
            read_file(fs, node).map_err(|e| format!("{when}: {}", explain(&item.path, e)))?;
        if data.len() as u64 != item.len || pbr_crc::crc32(&data) != item.crc {
            return Err(format!("{when}: the contents of {} are wrong", item.path));
        }
    }
    let mut list = Vec::new();
    let root = fs.root();
    walk(fs, root, "", &mut list)?;
    if list.len() != model.items.len() {
        return Err(format!(
            "{when}: the volume holds {} items, expected {}",
            list.len(),
            model.items.len()
        ));
    }
    let problems = fs.check(false).map_err(|e| format!("{when}: check: {}", describe(e)))?;
    if let Some(p) = problems.first() {
        return Err(format!("{when}: check found: {}", p.text));
    }
    Ok(())
}

/// The folder a new item goes in, picked at random: the top or any existing folder.
fn pick_folder(r: &mut Rng, model: &Model) -> String {
    let mut all = vec![String::new()];
    all.extend(
        model.folders().into_iter().filter_map(|k| model.items.get(&k).map(|i| i.path.clone())),
    );
    r.pick(&all).clone()
}

fn depth(path: &str) -> usize {
    path.matches('/').count()
}

/// Makes the model agree with the volume again after something ran out of space part-way.
fn resync(fs: &mut Volume, model: &mut Model) -> Result<(), String> {
    *model = scan(fs)?;
    Ok(())
}

/// Why an operation stopped.
enum Stop {
    /// The volume ran out of space (not a failure: the volume may hold part of the change).
    Space,
    /// Something is wrong, described for the person running the tool.
    Failed(String),
}

impl From<String> for Stop {
    fn from(message: String) -> Stop {
        Stop::Failed(message)
    }
}

/// Turns a filesystem error on `path` into a [`Stop`].
fn fail_at(path: &str) -> impl Fn(FsError) -> Stop + '_ {
    move |e| if e == FsError::NoSpace { Stop::Space } else { Stop::Failed(explain(path, e)) }
}

/// Does one random operation. `Err` is a failure of the filesystem, not of space.
fn step(fs: &mut Volume, model: &mut Model, r: &mut Rng) -> Result<(), String> {
    let op = r.below(100);
    let files = model.files();
    let result: Result<(), Stop> = (|| {
        if op < 30 || files.is_empty() && op < 85 {
            // Create a file, or replace the contents of one with the same name.
            let folder = pick_folder(r, model);
            let path = format!("{folder}/{}", file_name(r));
            if model.items.get(&key(&path)).is_some_and(|i| i.folder) {
                return Ok(());
            }
            let data = contents(r.next(), *r.pick(&SIZES));
            let (dir, name) = resolve_parent(fs, &path)?;
            let exists = model.items.contains_key(&key(&path));
            let node = if exists {
                fs.lookup(dir, name).map_err(fail_at(&path))?
            } else {
                fs.create(dir, name, Kind::File).map_err(fail_at(&path))?
            };
            if exists && r.below(2) == 0 {
                fs.set_size(node, 0).map_err(fail_at(&path))?;
            }
            write_at(fs, node, 0, &data).map_err(fail_at(&path))?;
            fs.set_size(node, data.len() as u64).map_err(fail_at(&path))?;
            fs.commit(node).map_err(fail_at(&path))?;
            let spelled = if exists { model.items[&key(&path)].path.clone() } else { path };
            model.insert(Item {
                path: spelled,
                folder: false,
                len: data.len() as u64,
                crc: pbr_crc::crc32(&data),
            });
        } else if op < 40 {
            // Change part of a file, perhaps past its end.
            let k = r.pick(&files).clone();
            let item = model.items[&k].clone();
            let node = resolve(fs, &item.path)?;
            let mut data = read_file(fs, node).map_err(fail_at(&item.path))?;
            let at = r.below(data.len() as u64 + 3000) as usize;
            let patch = contents(r.next(), r.below(20_000) as usize + 1);
            if data.len() < at + patch.len() {
                data.resize(at + patch.len(), 0);
            }
            data[at..at + patch.len()].copy_from_slice(&patch);
            write_at(fs, node, at as u64, &patch).map_err(fail_at(&item.path))?;
            fs.commit(node).map_err(fail_at(&item.path))?;
            model.insert(Item { len: data.len() as u64, crc: pbr_crc::crc32(&data), ..item });
        } else if op < 45 {
            // Cut a file short, or stretch it with zeros.
            let k = r.pick(&files).clone();
            let item = model.items[&k].clone();
            let node = resolve(fs, &item.path)?;
            let mut data = read_file(fs, node).map_err(fail_at(&item.path))?;
            let size = r.below(data.len() as u64 + 9000) as usize;
            fs.set_size(node, size as u64).map_err(fail_at(&item.path))?;
            fs.commit(node).map_err(fail_at(&item.path))?;
            data.resize(size, 0);
            model.insert(Item { len: size as u64, crc: pbr_crc::crc32(&data), ..item });
        } else if op < 60 {
            // Remove a file, or an empty folder.
            let k = if r.below(5) == 0 {
                let empties: Vec<String> =
                    model.folders().into_iter().filter(|f| model.is_empty_folder(f)).collect();
                if empties.is_empty() { r.pick(&files).clone() } else { r.pick(&empties).clone() }
            } else {
                r.pick(&files).clone()
            };
            let path = model.items[&k].path.clone();
            let (dir, name) = resolve_parent(fs, &path)?;
            fs.remove(dir, name).map_err(fail_at(&path))?;
            model.items.remove(&k);
        } else if op < 68 {
            let parent = pick_folder(r, model);
            if depth(&parent) < 5 && model.folders().len() < 40 {
                let path = format!("{parent}/{}", folder_name(r));
                if model.items.contains_key(&key(&path)) {
                    return Ok(());
                }
                let (dir, name) = resolve_parent(fs, &path)?;
                fs.create(dir, name, Kind::Folder).map_err(fail_at(&path))?;
                model.insert(Item { path, folder: true, len: 0, crc: 0 });
            }
        } else if op < 88 {
            rename_one(fs, model, r)?;
        } else if !files.is_empty() {
            // Names match whatever the capitals: look a file up spelled all in capitals.
            let k = r.pick(&files).clone();
            let path = model.items[&k].path.clone();
            let shouted = path.to_ascii_uppercase();
            let a = resolve(fs, &path)?;
            let b = resolve(fs, &shouted)?;
            if a != b {
                return Err(Stop::Failed(format!(
                    "{path}: the capitals changed which file was found"
                )));
            }
        }
        Ok(())
    })();
    match result {
        Ok(()) => Ok(()),
        Err(Stop::Space) => resync(fs, model),
        Err(Stop::Failed(message)) => Err(message),
    }
}

/// Moves a random file or folder to a random folder under a new name.
fn rename_one(fs: &mut Volume, model: &mut Model, r: &mut Rng) -> Result<(), Stop> {
    let all: Vec<String> = model.items.keys().cloned().collect();
    if all.is_empty() {
        return Ok(());
    }
    let from_key = r.pick(&all).clone();
    let from = model.items[&from_key].clone();
    let folder = pick_folder(r, model);
    let name = if from.folder { folder_name(r) } else { file_name(r) };
    let to = format!("{folder}/{name}");
    let to_key = key(&to);
    let inside_itself =
        from.folder && (to_key == from_key || to_key.starts_with(&format!("{from_key}/")));
    let replace = !from.folder && r.below(2) == 0;
    let existing = model.items.get(&to_key).cloned();
    let (from_dir, from_name) = resolve_parent(fs, &from.path)?;
    let (to_dir, to_name) = resolve_parent(fs, &to)?;
    let outcome = fs.rename(from_dir, from_name, to_dir, to_name, replace);
    // What the rules say should happen.
    let expect = if inside_itself {
        Some(FsError::BadArgument)
    } else if to_key == from_key {
        None
    } else {
        match &existing {
            Some(e) if !(replace && !e.folder && !from.folder) => Some(FsError::AlreadyExists),
            _ => None,
        }
    };
    match (outcome, expect) {
        (Err(FsError::NoSpace), _) => Err(Stop::Space),
        (Err(e), Some(want)) if e == want => Ok(()),
        (Err(e), _) => Err(fail_at(&format!("rename {} to {to}", from.path))(e)),
        (Ok(()), Some(_)) => {
            Err(Stop::Failed(format!("rename {} to {to} should have been refused", from.path)))
        }
        (Ok(()), None) => {
            // Move the item and everything below it in the model.
            let prefix = format!("{from_key}/");
            let moved: Vec<String> =
                model.items.keys().filter(|k| k.starts_with(&prefix)).cloned().collect();
            let mut carried = Vec::new();
            for k in moved {
                if let Some(mut it) = model.items.remove(&k) {
                    it.path = format!("{to}{}", &it.path[from.path.len()..]);
                    carried.push(it);
                }
            }
            model.items.remove(&from_key);
            model.items.remove(&to_key);
            model.insert(Item { path: to, ..from });
            for it in carried {
                model.insert(it);
            }
            Ok(())
        }
    }
}

/// The manifest text for a model.
fn manifest(model: &Model) -> String {
    let mut out = String::new();
    for item in model.items.values() {
        let kind = if item.folder { 'D' } else { 'F' };
        let _ = writeln!(out, "{kind} {} {:08x} {}", item.len, item.crc, item.path);
    }
    out
}

/// `stress SEED ROUNDS MANIFEST`
pub fn stress(image: &str, args: &[String]) -> Outcome {
    let [seed, rounds, out] = args else {
        return Err("stress needs SEED ROUNDS MANIFEST".into());
    };
    let seed: u64 = seed.parse().map_err(|_| format!("{seed}: not a number"))?;
    let rounds: u64 = rounds.parse().map_err(|_| format!("{rounds}: not a number"))?;
    let mut fs = mount(image, true)?;
    let mut model = scan(&mut fs)?;
    let mut r = Rng::new(seed);
    for round in 0..rounds {
        step(&mut fs, &mut model, &mut r).map_err(|e| format!("round {round}: {e}"))?;
        if round % 100 == 99 {
            verify_model(&mut fs, &model, &format!("after round {}", round + 1))?;
        }
    }
    verify_model(&mut fs, &model, "at the end")?;
    close(fs)?;
    std::fs::write(out, manifest(&model)).map_err(|e| format!("{out}: {e}"))?;
    let files = model.items.values().filter(|i| !i.folder).count();
    println!("stress: {rounds} rounds, {files} files and {} folders", model.items.len() - files);
    Ok(0)
}

/// One line of a manifest.
struct Line {
    folder: bool,
    len: u64,
    crc: u32,
    path: String,
}

fn parse_manifest(text: &str) -> Result<(Vec<Line>, bool), String> {
    let mut lines = Vec::new();
    let mut full = false;
    for (n, line) in text.lines().enumerate() {
        if line.is_empty() {
            continue;
        }
        let bad = || format!("manifest line {}: not understood", n + 1);
        let mut fields = line.splitn(4, ' ');
        let first = fields.next().ok_or_else(bad)?;
        let (folder, len_text) = match first {
            "F" | "D" => {
                full = true;
                (first == "D", fields.next().ok_or_else(bad)?)
            }
            _ => (false, first),
        };
        let len = len_text.parse().map_err(|_| bad())?;
        let crc = u32::from_str_radix(fields.next().ok_or_else(bad)?, 16).map_err(|_| bad())?;
        let path = fields.next().ok_or_else(bad)?.to_string();
        lines.push(Line { folder, len, crc, path });
    }
    Ok((lines, full))
}

/// `verify [--exact] MANIFEST`: every file in the manifest must be there with the right size
/// and checksum. With `--exact` the volume must hold nothing else (only for full manifests).
pub fn verify(image: &str, args: &[String]) -> Outcome {
    let exact = args.iter().any(|a| a == "--exact");
    let path = args.iter().find(|a| *a != "--exact").ok_or("verify needs a manifest")?;
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let (lines, full) = parse_manifest(&text)?;
    let mut fs = mount(image, false)?;
    let (mut files, mut folders) = (0, 0);
    for line in &lines {
        let node = resolve(&mut fs, &line.path)?;
        let st = fs.stat(node).map_err(|e| explain(&line.path, e))?;
        if line.folder != (st.kind == Kind::Folder) {
            return Err(format!(
                "{}: expected a {}",
                line.path,
                if line.folder { "folder" } else { "file" }
            ));
        }
        if line.folder {
            folders += 1;
            continue;
        }
        let data = read_file(&mut fs, node).map_err(|e| explain(&line.path, e))?;
        if data.len() as u64 != line.len || pbr_crc::crc32(&data) != line.crc {
            return Err(format!("{} differs from the manifest", line.path));
        }
        files += 1;
    }
    if exact {
        if !full {
            return Err("--exact needs a manifest that lists folders too".into());
        }
        let mut all = Vec::new();
        let root = fs.root();
        walk(&mut fs, root, "", &mut all)?;
        if all.len() != lines.len() {
            return Err(format!(
                "the volume holds {} items, the manifest lists {}",
                all.len(),
                lines.len()
            ));
        }
    }
    close(fs)?;
    println!("verified {files} files and {folders} folders");
    Ok(0)
}
