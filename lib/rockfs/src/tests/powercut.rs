// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Simulated power cuts (docs/design/0016-filesystems.md, section 4, layer 1): the test of
//! RockFS's promise that after a crash every file is exactly as of its last commit (or of the
//! commit under way), and every folder change either happened or didn't.
//!
//! A real disk has a *write cache*: it says a write is done when the data is in its memory,
//! and puts it on the platters or flash later, in whatever order suits it. Only a *flush*
//! promises that everything written before it is really stored. [`CutDisk`] behaves like that,
//! as harshly as a disk may: it keeps what has been flushed apart from what has been written
//! since, and when the power goes (at a chosen write) each sector written since the last flush
//! independently survives or is lost. Then it stops answering, as a machine without power
//! does.
//!
//! Each run makes a volume, sets up a few folders and files, then does random operations until
//! the power goes. The disk as it was left is mounted again (which checks and repairs it, since
//! it wasn't shut down cleanly), and must then:
//! - check clean;
//! - hold exactly what it held just before the operation the power cut, or just after it;
//!   when the cut came during an unmount (which commits every file in turn), each file may be
//!   either;
//! - keep working: a new file can be written, and after a clean unmount and mount it checks
//!   clean again.
//!
//! The one thing a crash may add is the `/lost+found` folder: a file whose name was removed
//! but whose inode wasn't yet freed is put there by the repair. That's allowed only when the
//! operation cut short was removing (or replacing) a file, and the file found must be that one.

use super::*;
use pbr_fs::{DeviceError, SECTOR};
use std::collections::BTreeMap;
use std::format;
use std::string::ToString;

/// A disk with a write cache, whose power can be cut. Its state is shared (`Rc<RefCell<..>>`:
/// a pointer with shared ownership, to something that can be changed through any copy of it),
/// so the test keeps hold of the disk even when a mount that fails has used up the device.
#[derive(Clone)]
struct CutDisk(std::rc::Rc<core::cell::RefCell<Disk>>);

struct Disk {
    /// What is safely stored: everything up to the last flush, and (after a cut) whatever of
    /// the rest survived.
    stored: Vec<u8>,
    /// Sectors written since the last flush, in order: (sector, contents).
    cached: Vec<(u64, Vec<u8>)>,
    /// The power goes during the write after this many more; `None` for never.
    writes_left: Option<u64>,
    /// Decides which cached sectors survive the cut.
    rng: Rng,
    /// Whether the power has gone.
    dead: bool,
}

impl CutDisk {
    fn new(disk: MemDisk, seed: u64) -> CutDisk {
        CutDisk(std::rc::Rc::new(core::cell::RefCell::new(Disk {
            stored: disk.data,
            cached: Vec::new(),
            writes_left: None,
            rng: Rng(seed | 1),
            dead: false,
        })))
    }
    fn dead(&self) -> bool {
        self.0.borrow().dead
    }
    fn cut_after(&self, writes: u64) {
        self.0.borrow_mut().writes_left = Some(writes);
    }
    fn image(&self) -> MemDisk {
        MemDisk::from(self.0.borrow().stored.clone())
    }
}

impl Disk {
    /// The power goes: each cached sector survives with even chances, and the survivors land
    /// in the order they were written (a sector written twice ends up with one of its two
    /// versions, or the older stored one).
    fn cut(&mut self) {
        for (sector, data) in core::mem::take(&mut self.cached) {
            if self.rng.below(2) == 0 {
                let at = sector as usize * SECTOR;
                self.stored[at..at + SECTOR].copy_from_slice(&data);
            }
        }
        self.dead = true;
    }
}

impl BlockDevice for CutDisk {
    fn sectors(&self) -> u64 {
        (self.0.borrow().stored.len() / SECTOR) as u64
    }
    fn read_only(&self) -> bool {
        false
    }
    fn read(&mut self, sector: u64, buf: &mut [u8]) -> Result<(), DeviceError> {
        self.0.borrow_mut().read(sector, buf)
    }
    fn write(&mut self, sector: u64, data: &[u8]) -> Result<(), DeviceError> {
        self.0.borrow_mut().write(sector, data)
    }
    fn flush(&mut self) -> Result<(), DeviceError> {
        self.0.borrow_mut().flush()
    }
}

impl Disk {
    fn read(&mut self, sector: u64, buf: &mut [u8]) -> Result<(), DeviceError> {
        if self.dead {
            return Err(DeviceError);
        }
        let at = sector as usize * SECTOR;
        buf.copy_from_slice(&self.stored[at..at + buf.len()]);
        // The disk answers reads from its cache: the newest version of each sector.
        for (s, data) in &self.cached {
            let off = *s as usize * SECTOR;
            if off >= at && off < at + buf.len() {
                buf[off - at..off - at + SECTOR].copy_from_slice(data);
            }
        }
        Ok(())
    }
    fn write(&mut self, sector: u64, data: &[u8]) -> Result<(), DeviceError> {
        if self.dead {
            return Err(DeviceError);
        }
        if let Some(n) = &mut self.writes_left {
            if *n == 0 {
                self.cut();
                return Err(DeviceError);
            }
            *n -= 1;
        }
        for (i, chunk) in data.chunks(SECTOR).enumerate() {
            self.cached.push((sector + i as u64, chunk.to_vec()));
        }
        Ok(())
    }
    fn flush(&mut self) -> Result<(), DeviceError> {
        if self.dead {
            return Err(DeviceError);
        }
        for (sector, data) in core::mem::take(&mut self.cached) {
            let at = sector as usize * SECTOR;
            self.stored[at..at + SECTOR].copy_from_slice(&data);
        }
        Ok(())
    }
}

/// What is at a path.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Item {
    Folder,
    File(Vec<u8>),
}

/// The volume as it should be: what is durable (as of the last commit or folder change), and
/// what each file holds now, including changes not committed.
#[derive(Clone, Default)]
struct Model {
    durable: BTreeMap<String, Item>,
    current: BTreeMap<String, Vec<u8>>,
}

impl Model {
    fn folders(&self) -> Vec<String> {
        let mut f: Vec<String> = std::vec![String::new()];
        f.extend(self.durable.iter().filter(|(_, i)| **i == Item::Folder).map(|(p, _)| p.clone()));
        f
    }
    fn files(&self) -> Vec<String> {
        self.current.keys().cloned().collect()
    }
}

/// Everything on a mounted volume, by path (`/lost+found` apart).
fn contents(fs: &mut RockFs<MemDisk>) -> (BTreeMap<String, Item>, Vec<(String, Item)>) {
    fn walk(fs: &mut RockFs<MemDisk>, path: &str, node: NodeId, out: &mut BTreeMap<String, Item>) {
        for name in names(fs, node) {
            let p = format!("{path}/{name}");
            let child = fs.lookup(node, &name).unwrap();
            if fs.stat(child).unwrap().kind == Kind::Folder {
                out.insert(p.clone(), Item::Folder);
                walk(fs, &p, child, out);
            } else {
                out.insert(p, Item::File(read_all(fs, child).unwrap()));
            }
        }
    }
    let mut all = BTreeMap::new();
    let root = fs.root();
    walk(fs, "", root, &mut all);
    let lost: Vec<(String, Item)> = all
        .iter()
        .filter(|(p, _)| p.starts_with("/lost+found/"))
        .map(|(p, i)| (p.clone(), i.clone()))
        .collect();
    all.retain(|p, _| p != "/lost+found" && !p.starts_with("/lost+found/"));
    (all, lost)
}

/// An operation, and how the durable state may look if the power goes during it.
enum Outcome {
    /// Nothing durable changed (or the operation failed without changing anything).
    Same,
    /// The durable state is either the old one or `after`; `removed` is the file a crash may
    /// leave in `/lost+found`.
    Either { after: BTreeMap<String, Item>, removed: Option<Vec<u8>> },
    /// Each file is independently as before or as `after` (an unmount committing everything).
    EachFile { after: BTreeMap<String, Item> },
}

/// Does one random operation, updating the model if it succeeds. Returns what the power cut
/// may have left, and whether the operation reached the disk's end (failed with `Disk`).
fn step(fs: &mut RockFs<CutDisk>, model: &mut Model, rng: &mut Rng) -> (Outcome, bool) {
    let files = model.files();
    let folders = model.folders();
    let pick = |rng: &mut Rng, v: &[String]| v[rng.below(v.len() as u64) as usize].clone();
    let node = |fs: &mut RockFs<CutDisk>, path: &str| -> NodeId {
        let mut n = fs.root();
        for part in path.split('/').filter(|p| !p.is_empty()) {
            n = fs.lookup(n, part).unwrap();
        }
        n
    };
    let names = ["a", "b", "c", "d", "Long name with spaces.txt", "é"];
    let fresh_name = |rng: &mut Rng, folder: &str, model: &Model| -> Option<String> {
        let p = format!("{folder}/{}", names[rng.below(names.len() as u64) as usize]);
        let taken = model.durable.keys().any(|k| k.eq_ignore_ascii_case(&p));
        (!taken).then_some(p)
    };
    let failed = |r: Result<(), FsError>| -> Option<bool> {
        match r {
            Ok(()) => None,
            Err(FsError::Disk) => Some(true),
            Err(_) => Some(false),
        }
    };
    let choice = rng.below(100);
    if choice < 35 && !files.is_empty() {
        // Change a file's contents (pending; nothing durable changes).
        let path = pick(rng, &files);
        let n = node(fs, &path);
        let mut data = model.current[&path].clone();
        let r = if rng.below(4) == 0 {
            let size = rng.below(60_000) as usize;
            data.resize(size, 0);
            fs.set_size(n, size as u64)
        } else {
            let off = rng.below(data.len() as u64 + 8192) as usize;
            let new = bytes(rng.next(), rng.below(50_000) as usize + 1);
            if data.len() < off + new.len() {
                data.resize(off + new.len(), 0);
            }
            data[off..off + new.len()].copy_from_slice(&new);
            fs.write(n, off as u64, &new).map(|_| ())
        };
        match failed(r) {
            None => {
                model.current.insert(path, data);
            }
            Some(dead) => return (Outcome::Same, dead),
        }
        (Outcome::Same, false)
    } else if choice < 60 && !files.is_empty() {
        // Commit a file.
        let path = pick(rng, &files);
        let mut after = model.durable.clone();
        after.insert(path.clone(), Item::File(model.current[&path].clone()));
        let n = node(fs, &path);
        match failed(fs.commit(n)) {
            None => model.durable = after.clone(),
            Some(dead) => return (Outcome::Either { after, removed: None }, dead),
        }
        (Outcome::Same, false)
    } else if choice < 75 {
        // Create a file or a folder.
        let folder = pick(rng, &folders);
        let Some(path) = fresh_name(rng, &folder, model) else { return (Outcome::Same, false) };
        let folder_kind = rng.below(3) == 0;
        let mut after = model.durable.clone();
        after.insert(path.clone(), if folder_kind { Item::Folder } else { Item::File(Vec::new()) });
        let parent = node(fs, &folder);
        let name = path.rsplit_once('/').unwrap().1;
        let kind = if folder_kind { Kind::Folder } else { Kind::File };
        match failed(fs.create(parent, name, kind).map(|_| ())) {
            None => {
                model.durable = after;
                if !folder_kind {
                    model.current.insert(path, Vec::new());
                }
            }
            Some(dead) => return (Outcome::Either { after, removed: None }, dead),
        }
        (Outcome::Same, false)
    } else if choice < 85 && !files.is_empty() {
        // Remove a file.
        let path = pick(rng, &files);
        let mut after = model.durable.clone();
        let removed = match after.remove(&path) {
            Some(Item::File(d)) => d,
            _ => unreachable!(),
        };
        let (folder, name) = path.rsplit_once('/').unwrap();
        let parent = node(fs, folder);
        match failed(fs.remove(parent, name)) {
            None => {
                model.durable = after;
                model.current.remove(&path);
            }
            Some(dead) => return (Outcome::Either { after, removed: Some(removed) }, dead),
        }
        (Outcome::Same, false)
    } else if choice < 97 && !files.is_empty() {
        // Rename a file into any folder, sometimes over another file.
        let path = pick(rng, &files);
        let to = if rng.below(3) == 0 && files.len() > 1 {
            pick(rng, &files)
        } else {
            let folder = pick(rng, &folders);
            match fresh_name(rng, &folder, model) {
                Some(p) => p,
                None => return (Outcome::Same, false),
            }
        };
        if to == path || model.durable.keys().any(|k| k != &to && k.eq_ignore_ascii_case(&to)) {
            return (Outcome::Same, false);
        }
        let mut after = model.durable.clone();
        let moving = after.remove(&path).unwrap();
        let replaced = match after.insert(to.clone(), moving) {
            Some(Item::File(d)) => Some(d),
            _ => None,
        };
        let (from_dir, from_name) = path.rsplit_once('/').unwrap();
        let (to_dir, to_name) = to.rsplit_once('/').unwrap();
        let (a, b) = (node(fs, from_dir), node(fs, to_dir));
        match failed(fs.rename(a, from_name, b, to_name, true)) {
            None => {
                model.durable = after;
                let data = model.current.remove(&path).unwrap();
                model.current.insert(to, data);
            }
            Some(dead) => return (Outcome::Either { after, removed: replaced }, dead),
        }
        (Outcome::Same, false)
    } else {
        // Unmount (committing every file) and mount again.
        let mut after = model.durable.clone();
        for (p, d) in &model.current {
            after.insert(p.clone(), Item::File(d.clone()));
        }
        match fs.unmount() {
            Ok(()) => model.durable = after,
            Err(FsError::Disk) => return (Outcome::EachFile { after }, true),
            Err(e) => panic!("unmount: {e:?}"),
        }
        (Outcome::Same, false)
    }
}

/// One run: set up, work until the power goes, and check what's left.
fn run(seed: u64) {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9) | 1);
    let disk = CutDisk::new(formatted(400), seed);
    let mut fs = RockFs::mount(disk.clone(), clock, false).unwrap();
    let mut model = Model::default();
    // A few folders and files to start from, all durable.
    let root = fs.root();
    for f in ["/Documents", "/Documents/Old"] {
        let (dir, name) = f.rsplit_once('/').unwrap();
        let parent = if dir.is_empty() { root } else { fs.lookup(root, &dir[1..]).unwrap() };
        fs.create(parent, name, Kind::Folder).unwrap();
        model.durable.insert(f.to_string(), Item::Folder);
    }
    for (i, f) in ["/a", "/Documents/b", "/Documents/Old/c"].iter().enumerate() {
        let data = bytes(seed + i as u64, (rng.below(80_000)) as usize);
        let (dir, name) = f.rsplit_once('/').unwrap();
        let mut parent = root;
        for part in dir.split('/').filter(|p| !p.is_empty()) {
            parent = fs.lookup(parent, part).unwrap();
        }
        let n = fs.create(parent, name, Kind::File).unwrap();
        for (k, chunk) in data.chunks(65536).enumerate() {
            fs.write(n, k as u64 * 65536, chunk).unwrap();
        }
        fs.commit(n).unwrap();
        model.durable.insert(f.to_string(), Item::File(data.clone()));
        model.current.insert(f.to_string(), data);
    }

    // The power goes at a random write, somewhere in the next few dozen operations.
    disk.cut_after(rng.below(400));
    let mut ops = 0;
    let (outcome, before) = loop {
        let before = model.durable.clone();
        let (outcome, dead) = step(&mut fs, &mut model, &mut rng);
        if disk.dead() {
            assert!(dead, "seed {seed}: the power went but the operation said it worked");
            break (outcome, before);
        }
        ops += 1;
        if ops > 2000 {
            return; // the cut came after the work ended: nothing to look at
        }
        if fs.read_only() {
            // An unmount succeeded: mount again on the same disk. The power may go during
            // the mount, which changes nothing a file holds.
            match RockFs::mount(disk.clone(), clock, false) {
                Ok(again) => fs = again,
                Err(_) if disk.dead() => break (Outcome::Same, model.durable.clone()),
                Err(e) => panic!("seed {seed}: mounting again: {e:?}"),
            }
        }
    };

    // Mount what was left, as the next boot would.
    let image = disk.image();
    let mut fs = match RockFs::mount(image, clock, false) {
        Ok(fs) => fs,
        Err(e) => panic!("seed {seed}: after the power cut the volume won't mount: {e:?}"),
    };
    let problems = fs.check(false).unwrap();
    assert!(problems.is_empty(), "seed {seed}: after the repair at mount: {problems:#?}");
    let (found, lost) = contents(&mut fs);
    let ok = match &outcome {
        Outcome::Same => found == before,
        Outcome::Either { after, .. } => found == before || found == *after,
        Outcome::EachFile { after } => {
            found.len() == before.len()
                && found
                    .iter()
                    .all(|(p, item)| before.get(p) == Some(item) || after.get(p) == Some(item))
        }
    };
    assert!(
        ok,
        "seed {seed}: after the power cut the volume holds\n{:?}\nnot what it held before:\n{:?}\n{}",
        found.keys().collect::<Vec<_>>(),
        before.keys().collect::<Vec<_>>(),
        match &outcome {
            Outcome::Same => "(a change that wasn't durable yet)".to_string(),
            Outcome::Either { after, removed } => format!(
                "or what it would have held after (removing {:?} bytes):\n{:?}",
                removed.as_ref().map(Vec::len),
                after.keys().collect::<Vec<_>>()
            ),
            Outcome::EachFile { .. } => "(an unmount)".to_string(),
        }
    );
    // Only a file being removed or replaced may turn up in `/lost+found`, and it must be that
    // file, whole.
    for (path, item) in &lost {
        let allowed = match &outcome {
            Outcome::Either { removed: Some(d), .. } => item == &Item::File(d.clone()),
            _ => false,
        };
        assert!(allowed, "seed {seed}: {path} turned up in lost+found");
    }
    // And the volume carries on working.
    put(&mut fs, "/after the cut", b"still works").unwrap();
    let mut fs = remount(fs);
    assert_clean(&mut fs);
    let n = node_at(&mut fs, "/after the cut").unwrap();
    assert_eq!(read_all(&mut fs, n).unwrap(), b"still works");
}

#[test]
fn power_cuts_never_leave_a_file_half_old_and_half_new() {
    // 300 runs normally; `PBR_POWER_CUTS=N` runs N, for a longer search.
    let runs = std::env::var("PBR_POWER_CUTS").ok().and_then(|v| v.parse().ok()).unwrap_or(300);
    for seed in 0..runs {
        run(seed);
    }
}

#[test]
fn the_cut_disk_loses_only_what_was_not_flushed() {
    // The test device itself: flushed writes always survive; unflushed ones may not.
    let mut d = CutDisk::new(MemDisk::new(16), 1);
    d.write(0, &[1; SECTOR]).unwrap();
    d.flush().unwrap();
    for s in 1..16 {
        d.write(s, &[2; SECTOR]).unwrap();
    }
    d.cut_after(0);
    assert!(d.write(0, &[3; SECTOR]).is_err());
    assert!(d.read(0, &mut [0; SECTOR]).is_err(), "a disk without power doesn't answer");
    let img = d.image();
    assert_eq!(&img.data[..SECTOR], &[1; SECTOR][..]);
    let survived = (1..16).filter(|s| img.data[s * SECTOR] == 2).count();
    assert!(survived > 0 && survived < 15, "{survived} of 15 unflushed sectors survived");
}
